//! keyrock-case-study client — a demonstration terminal viewer.
//!
//! Connects to the gRPC server (`src/server.rs`) and redraws the combined
//! book in place, like `top`. This is not part of the service — it exists
//! so a reviewer can see the merged book without `grpcurl`, and so step 7's
//! reconnection/staleness work has an instrument that makes it visible on
//! screen rather than only in logs.
//!
//! Reachable via `keyrock_case_study::orderbook`, the generated proto module
//! `src/lib.rs` already re-exports — no library change was needed to add
//! this binary.

use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use clap::Parser;
use keyrock_case_study::exchange::Venue;
use keyrock_case_study::orderbook::orderbook_aggregator_client::OrderbookAggregatorClient;
use keyrock_case_study::orderbook::{Empty, Level, Summary};
use keyrock_case_study::telemetry;
use tokio::time::sleep;
use tokio_stream::StreamExt;
use tracing::{info, warn};

/// Rows rendered per side. Fewer real levels than this leave the remaining
/// rows blank rather than collapsing the layout, so nothing jumps between
/// frames.
const ROWS: usize = 10;

/// Both venues this project drives, declared once here rather than
/// hardcoded into the header string — so step 7 can attach per-venue status
/// (see spec.md's "design for step 7") by extending what `render` does with
/// this list, not by restructuring its signature.
const VENUES: [Venue; 3] = [Venue::Binance, Venue::Bitstamp, Venue::Kraken];

#[derive(Parser)]
#[command(name = "client", version, about = "keyrock-case-study demo client")]
struct Cli {
    /// gRPC server address, e.g. http://127.0.0.1:50051.
    #[arg(long, default_value = "http://127.0.0.1:50051")]
    addr: String,
}

/// Tracks what the render loop needs across frames: how many `Summary`
/// messages have arrived, and a rolling messages/second rate.
struct Stats {
    count: u64,
    rate: f64,
    /// When `rate` was last recomputed, and the count at that time — the
    /// rolling rate is `(count - count_at_last_calc) / elapsed`.
    last_calc: Instant,
    count_at_last_calc: u64,
}

impl Stats {
    fn new() -> Self {
        Self {
            count: 0,
            rate: 0.0,
            last_calc: Instant::now(),
            count_at_last_calc: 0,
        }
    }

    /// Records one received message and, if enough time has passed,
    /// recomputes the rolling rate.
    ///
    /// Guarded against a near-zero elapsed interval: on the very first call
    /// (or any call arriving in the same instant as the last recompute),
    /// `elapsed` can be ~0, and `count / elapsed` would print `inf`/`NaN` on
    /// screen. Holding the last computed rate (0.0 initially) until a
    /// meaningful interval has passed avoids that without a special case at
    /// render time.
    fn record(&mut self) {
        self.count += 1;
        let elapsed = self.last_calc.elapsed().as_secs_f64();
        if elapsed > 0.2 {
            self.rate = (self.count - self.count_at_last_calc) as f64 / elapsed;
            self.count_at_last_calc = self.count;
            self.last_calc = Instant::now();
        }
    }
}

/// Client-side per-venue last-seen-in-a-frame tracking, used to render the
/// header's `● ` / `○ stale <Ns>` status.
///
/// This is **not** the server's staleness state from step 7's aggregator
/// (`src/aggregator.rs`'s `Venue::staleness_threshold`) — `Summary` carries
/// no venue-health field, and the `.proto` stays untouched (see spec.md's
/// Invariants), so the client can only infer health from which venues'
/// levels actually show up in each streamed frame. The two numbers measure
/// different events (server: last message *received* from the venue;
/// client: last *frame* whose top-10 contained that venue's levels) and the
/// client's figure therefore runs slightly behind the server's — by roughly
/// one publish interval — not because either is wrong. See README's
/// "Client-side status" note.
struct VenueTracker {
    last_seen: HashMap<Venue, Instant>,
}

impl VenueTracker {
    /// Seeds every tracked venue with the current instant, so the header
    /// prints a small, honest "stale 0.0s" rather than a special-cased blank
    /// before the first frame arrives.
    fn new(venues: &[Venue]) -> Self {
        let now = Instant::now();
        Self {
            last_seen: venues.iter().map(|v| (*v, now)).collect(),
        }
    }

    /// Records which of `venues` appear in `summary`'s levels this frame,
    /// bumping their last-seen instant, and returns that set for the render
    /// step to use for `●` vs `○`.
    fn update(&mut self, venues: &[Venue], summary: &Summary) -> HashSet<Venue> {
        let now = Instant::now();
        let mut present = HashSet::new();
        for level in summary.bids.iter().chain(summary.asks.iter()) {
            for &v in venues {
                if v.to_string() == level.exchange {
                    present.insert(v);
                }
            }
        }
        for &v in &present {
            self.last_seen.insert(v, now);
        }
        present
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    telemetry::init("info");

    let colour = std::io::stdout().is_terminal();
    let mut stats = Stats::new();
    let mut tracker = VenueTracker::new(&VENUES);

    // Fixed one-second-delay reconnect loop — deliberately simpler than
    // step 7's feed backoff (exponential + jitter). This one loop covers
    // both "server not listening yet" (compose's `depends_on` waits for the
    // container to start, not for the port to accept) and "server died and
    // came back."
    let reconnect_loop = async {
        loop {
            match connect_and_stream(&cli.addr, colour, &mut stats, &mut tracker).await {
                Ok(()) => info!("stream ended, reconnecting"),
                Err(e) => warn!(%e, "connect failed, retrying"),
            }
            sleep(Duration::from_secs(1)).await;
        }
    };

    // Explicit handler, not relying on the OS default SIGINT disposition:
    // this binary runs as PID 1 in the `client` container (exec-form
    // `ENTRYPOINT`/`entrypoint:`, no shell in between), and Linux does not
    // apply a signal's default action to PID 1 unless the process installs
    // its own handler for it — PID 1 silently ignores an unhandled SIGINT
    // rather than terminating. `tokio::signal::ctrl_c()` installs one, so
    // Ctrl-C exits both in a container and when run directly with `cargo
    // run`. Confirmed the un-fixed binary really did swallow `SIGINT` as
    // PID 1 (`docker kill --signal SIGINT` on a running container left it
    // up) before adding this.
    tokio::select! {
        _ = reconnect_loop => {}
        _ = tokio::signal::ctrl_c() => {
            info!("received ctrl-c, exiting");
        }
    }

    Ok(())
}

/// Connects once, streams `Summary` messages until the stream ends or
/// errors, rendering each one. Returns once the stream is exhausted (the
/// caller's loop reconnects).
async fn connect_and_stream(
    addr: &str,
    colour: bool,
    stats: &mut Stats,
    tracker: &mut VenueTracker,
) -> Result<()> {
    let mut client = OrderbookAggregatorClient::connect(addr.to_string()).await?;
    let mut stream = client.book_summary(Empty {}).await?.into_inner();

    let mut stdout = std::io::stdout();
    while let Some(summary) = stream.next().await {
        let summary = summary?;
        stats.record();
        let present = tracker.update(&VENUES, &summary);
        let frame = render(&VENUES, tracker, &present, &summary, stats, colour);
        stdout.write_all(frame.as_bytes())?;
        stdout.flush()?;
    }
    Ok(())
}

/// Table interior width in visible columns. `row()` pads every row to
/// exactly this width, which is also what lets the redraw skip a
/// clear-to-end-of-line: every frame writes the same total width, so
/// there's nothing left over from a longer previous frame to clear.
///
/// Total printed row width is `TABLE_WIDTH + 4` (borders + padding) = 76,
/// a few columns under the 80-column terminal a `docker compose up`-style
/// viewer typically assumes — deliberate margin, not 80 exactly, since an
/// exact-width line's wrap behaviour at the last column is inconsistent
/// across terminals, and `docker compose`'s own `"servicename | "` log
/// prefix already eats into the same budget when this isn't viewed with
/// `--no-log-prefix` (see `compose.yml`'s comment on the `client` service).
const TABLE_WIDTH: usize = 72;

/// Builds one full frame as a boxed table: cursor home once, then borders
/// and fixed-width rows — never a full-screen clear (`\x1b[2J`), which would
/// flicker at update rates this fast.
///
/// `colour` gates the cursor-home escape and the cell colour codes; the
/// box-drawing border characters are plain text either way, so a redirected
/// run still gets a readable framed table, just with no `\x1b[` sequence
/// anywhere in it — that's what makes "pipe stdout to a file and see clean
/// text" true.
///
/// Takes `venues` as a slice, plus `tracker`/`present` for per-venue status
/// (`●` for seen this frame, `○ stale <Ns>` otherwise) — step 7 fills in
/// this field rather than restructuring the header, per the shape step 6
/// set up for exactly this purpose.
fn render(
    venues: &[Venue],
    tracker: &VenueTracker,
    present: &HashSet<Venue>,
    summary: &Summary,
    stats: &Stats,
    colour: bool,
) -> String {
    let mut out = String::new();
    if colour {
        out.push_str("\x1b[H");
    }

    border(&mut out, '┌', '┐');
    // Padded manually rather than via `format!("{:<56}", ...)`: the status
    // string can carry ANSI colour codes, and `format!`'s width padding
    // counts raw bytes, which would misalign the border against a coloured
    // cell the same way `row()`'s own `visible_len` helper exists to avoid.
    let status = venue_status(venues, tracker, present, colour);
    let pad = 56usize.saturating_sub(visible_len(&status));
    row(
        &mut out,
        &format!("{status}{}{:>16}", " ".repeat(pad), now_hms()),
    );
    border(&mut out, '├', '┤');
    row(&mut out, &format!("{:^35} {:^35}", "BIDS", "ASKS"));
    border(&mut out, '├', '┤');

    // One shared max across *both* sides, not per-side — so a bid bar and
    // an ask bar of the same visual length really do represent the same
    // amount, at the cost of a lopsided book making one side's bars all
    // look small. Depth-bar convention (Binance/Bitstamp's own book UIs),
    // not this project's invention.
    let max_amount = summary
        .bids
        .iter()
        .chain(summary.asks.iter())
        .map(|l| l.amount)
        .fold(0.0_f64, f64::max);

    for i in 0..ROWS {
        let bid = summary.bids.get(i);
        let ask = summary.asks.get(i);
        row(
            &mut out,
            &format!(
                "{} {}",
                level_cell(bid, colour, "32", BAR_BG_BID, max_amount, FillFrom::Right),
                level_cell(ask, colour, "31", BAR_BG_ASK, max_amount, FillFrom::Left)
            ),
        );
    }

    border(&mut out, '├', '┤');
    row(&mut out, &spread_line(summary, stats, colour));
    border(&mut out, '└', '┘');

    out
}

/// Draws one border line, `TABLE_WIDTH + 2` dashes wide so it matches
/// `row`'s `"│ content │"` width exactly.
fn border(out: &mut String, left: char, right: char) {
    out.push(left);
    out.push_str(&"─".repeat(TABLE_WIDTH + 2));
    out.push(right);
    out.push('\n');
}

/// Wraps one row of content in the table's side borders, padded to
/// `TABLE_WIDTH` on *visible* width — `visible_len` skips any ANSI colour
/// codes already embedded in `content`, so a coloured cell doesn't throw off
/// border alignment the way naive `str::len` padding would.
fn row(out: &mut String, content: &str) {
    let pad = TABLE_WIDTH.saturating_sub(visible_len(content));
    out.push_str("│ ");
    out.push_str(content);
    out.push_str(&" ".repeat(pad));
    out.push_str(" │\n");
}

/// Visible character count, skipping `\x1b[...<letter>`-style ANSI escape
/// sequences — needed so `row`'s padding lines up borders even when
/// `content` has colour codes embedded in it.
fn visible_len(s: &str) -> usize {
    let mut len = 0;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            len += 1;
        }
    }
    len
}

/// Total visible width of one bid/ask cell: `"{:>12.8} {:>13.8} {:<8}"`.
const CELL_WIDTH: usize = 35;

/// Where the price/amount region ends and the venue label begins, within a
/// cell — the two get different foreground treatment (`price_fg` vs dim).
const CELL_FG_SPLIT: usize = 26;

/// 256-colour depth-bar backgrounds, one per side, matching the side's
/// foreground colour (bid green, ask red) rather than a neutral grey — a
/// deliberately solid, dark shade of that colour (not a paler "tint") so
/// the bar reads clearly instead of a translucent-looking highlight.
const BAR_BG_BID: &str = "48;5;22";
const BAR_BG_ASK: &str = "48;5;52";

/// Which edge of the cell a depth bar grows from. Bids sit in the left
/// column and asks in the right, so filling bids from the right and asks
/// from the left makes both bars grow toward the shared border between the
/// two columns — the spread — instead of both growing left-to-right
/// regardless of side.
#[derive(Clone, Copy)]
enum FillFrom {
    Left,
    Right,
}

/// One bid or ask cell: price, amount, venue label, with a depth bar shaded
/// into the row's background — proportional to `level`'s amount against
/// `max_amount` (the largest amount across *both* sides currently
/// displayed, per this project's depth-bar convention: same bar length on
/// either side means the same size). `None` renders as blank padding of the
/// same width, so a side with fewer than 10 levels doesn't reflow the
/// layout.
fn level_cell(
    level: Option<&Level>,
    colour: bool,
    price_fg: &str,
    bar_bg: &str,
    max_amount: f64,
    fill_from: FillFrom,
) -> String {
    let Some(l) = level else {
        return " ".repeat(CELL_WIDTH);
    };
    let plain = format!("{:>12.8} {:>13.8} {:<8}", l.price, l.amount, l.exchange);
    if !colour {
        return plain;
    }

    let fill_len = if max_amount > 0.0 {
        ((l.amount / max_amount) * CELL_WIDTH as f64)
            .round()
            .clamp(0.0, CELL_WIDTH as f64) as usize
    } else {
        0
    };
    shade_cell(&plain, price_fg, bar_bg, fill_len, fill_from)
}

/// Wraps `plain` (exactly `CELL_WIDTH` visible chars) in ANSI codes, run by
/// run, combining two independent boundaries: `CELL_FG_SPLIT` (foreground
/// colour changes from `price_fg` to dim) and `fill_len` (background turns
/// from `bar_bg` to the terminal default, on the edge `fill_from` picks).
/// Each run re-states both its foreground and background explicitly — SGR
/// state otherwise persists across writes, so leaving either unstated at a
/// run boundary would bleed the previous run's colour into the next one.
fn shade_cell(
    plain: &str,
    price_fg: &str,
    bar_bg: &str,
    fill_len: usize,
    fill_from: FillFrom,
) -> String {
    let fill_len = fill_len.min(CELL_WIDTH);
    // `Left` shades [0, fill_len); `Right` shades [CELL_WIDTH - fill_len,
    // CELL_WIDTH) — the same length, anchored to the opposite edge.
    let (bar_start, bar_end) = match fill_from {
        FillFrom::Left => (0, fill_len),
        FillFrom::Right => (CELL_WIDTH - fill_len, CELL_WIDTH),
    };

    let mut boundaries = [
        0,
        CELL_FG_SPLIT.min(CELL_WIDTH),
        bar_start,
        bar_end,
        CELL_WIDTH,
    ];
    boundaries.sort_unstable();

    let chars: Vec<char> = plain.chars().collect();
    let mut out = String::new();
    for w in boundaries.windows(2) {
        let (start, end) = (w[0], w[1]);
        if start == end {
            continue;
        }
        let fg = if start < CELL_FG_SPLIT { price_fg } else { "2" };
        let bg = if start >= bar_start && start < bar_end {
            bar_bg
        } else {
            "49"
        };
        out.push_str(&format!("\x1b[{fg};{bg}m"));
        out.extend(&chars[start..end]);
    }
    out.push_str("\x1b[0m");
    out
}

/// The bottom summary line: raw spread, spread in basis points, running
/// message total, and the rolling update rate.
fn spread_line(summary: &Summary, stats: &Stats, colour: bool) -> String {
    let best_bid = summary.bids.first().map(|l| l.price);
    let best_ask = summary.asks.first().map(|l| l.price);
    let mid = match (best_bid, best_ask) {
        (Some(b), Some(a)) => Some((b + a) / 2.0),
        _ => None,
    };
    let bps = match mid {
        // `mid` can be exactly 0.0 in a hand-built or degenerate book;
        // guarding here avoids a division by zero showing up as `inf`/`NaN`
        // in the bps figure, the same class of defect flagged for the
        // update-rate guard above.
        Some(m) if m != 0.0 => format!("{:.1} bps", summary.spread / m * 10_000.0),
        _ => "n/a".to_string(),
    };

    // Padded to a fixed width *before* colourizing — `colourize` only wraps
    // the string in escape codes, so padding first keeps this field's
    // visible width constant regardless of colour, which is what lets
    // `row()`'s single trailing pad still land the right border in place.
    let spread_field = format!("{:<40}", format!("spread {:>12.8} ({bps})", summary.spread));
    let spread_field = if summary.spread < 0.0 {
        colourize(&spread_field, "\x1b[1;31m", colour)
    } else {
        spread_field
    };

    // `stats.rate` is always finite by construction (`Stats::record` only
    // ever divides by an elapsed interval already checked > 0.2s), but
    // guarding the display too costs one line and means a future change to
    // `Stats` can't silently reintroduce `inf`/`NaN` on screen.
    let rate_text = if stats.rate.is_finite() {
        format!("{:.1}/s", stats.rate)
    } else {
        "-.-/s".to_string()
    };

    format!("{spread_field}{:>8} updates   {rate_text:<8}", stats.count)
}

/// Wall-clock `HH:MM:SS`, UTC. Good enough for a demo timestamp — pulling in
/// a timezone-aware crate for a display-only clock in a tool with no tests
/// would be a new dependency this step doesn't need.
fn now_hms() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!(
        "{:02}:{:02}:{:02}",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60
    )
}

/// Renders each venue's status cell: `binance ●` (green) if its levels
/// appeared in the most recent frame, `bitstamp ○ stale 4.2s` (red) if not,
/// with the duration read from the client's own `tracker` — never from a
/// server-side signal, since `Summary` carries no venue-health field.
fn venue_status(
    venues: &[Venue],
    tracker: &VenueTracker,
    present: &HashSet<Venue>,
    colour: bool,
) -> String {
    let now = Instant::now();
    venues
        .iter()
        .map(|v| {
            if present.contains(v) {
                colourize(&format!("{v} \u{25cf}"), "\x1b[32m", colour)
            } else {
                let elapsed = tracker
                    .last_seen
                    .get(v)
                    .map_or(0.0, |t| now.duration_since(*t).as_secs_f64());
                colourize(
                    &format!("{v} \u{25cb} stale {elapsed:.1}s"),
                    "\x1b[31m",
                    colour,
                )
            }
        })
        .collect::<Vec<_>>()
        .join("  ")
}

fn colourize(text: &str, code: &str, colour: bool) -> String {
    if colour {
        format!("{code}{text}\x1b[0m")
    } else {
        text.to_string()
    }
}
