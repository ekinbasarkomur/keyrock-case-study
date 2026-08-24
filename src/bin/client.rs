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
const VENUES: [Venue; 2] = [Venue::Binance, Venue::Bitstamp];

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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    telemetry::init("info");

    let colour = std::io::stdout().is_terminal();
    let mut stats = Stats::new();

    // Fixed one-second-delay reconnect loop — deliberately simpler than
    // step 7's feed backoff (exponential + jitter). This one loop covers
    // both "server not listening yet" (compose's `depends_on` waits for the
    // container to start, not for the port to accept) and "server died and
    // came back."
    loop {
        match connect_and_stream(&cli.addr, colour, &mut stats).await {
            Ok(()) => info!("stream ended, reconnecting"),
            Err(e) => warn!(%e, "connect failed, retrying"),
        }
        sleep(Duration::from_secs(1)).await;
    }
}

/// Connects once, streams `Summary` messages until the stream ends or
/// errors, rendering each one. Returns once the stream is exhausted (the
/// caller's loop reconnects).
async fn connect_and_stream(addr: &str, colour: bool, stats: &mut Stats) -> Result<()> {
    let mut client = OrderbookAggregatorClient::connect(addr.to_string()).await?;
    let mut stream = client.book_summary(Empty {}).await?.into_inner();

    let mut stdout = std::io::stdout();
    while let Some(summary) = stream.next().await {
        let summary = summary?;
        stats.record();
        let frame = render(&VENUES, &summary, stats, colour);
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
/// Takes `venues` as a slice rather than baking a fixed header string in,
/// so step 7 can extend the header with per-venue status without changing
/// this function's shape.
fn render(venues: &[Venue], summary: &Summary, stats: &Stats, colour: bool) -> String {
    let mut out = String::new();
    if colour {
        out.push_str("\x1b[H");
    }

    let venue_names: Vec<String> = venues.iter().map(Venue::to_string).collect();

    border(&mut out, '┌', '┐');
    row(
        &mut out,
        &format!("{:<56}{:>16}", venue_names.join(" + "), now_hms()),
    );
    border(&mut out, '├', '┤');
    row(&mut out, &format!("{:^35} {:^35}", "BIDS", "ASKS"));
    border(&mut out, '├', '┤');

    for i in 0..ROWS {
        let bid = summary.bids.get(i);
        let ask = summary.asks.get(i);
        row(
            &mut out,
            &format!(
                "{} {}",
                level_cell(bid, colour, "\x1b[32m"),
                level_cell(ask, colour, "\x1b[31m")
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

/// One bid or ask cell: price, amount, venue label. `None` renders as
/// blank padding of the same width, so a side with fewer than 10 levels
/// doesn't reflow the layout.
fn level_cell(level: Option<&Level>, colour: bool, price_code: &str) -> String {
    match level {
        Some(l) => {
            let price_amount = format!("{:>12.8} {:>13.8}", l.price, l.amount);
            let price_amount = colourize(&price_amount, price_code, colour);
            let venue = format!(" {:<8}", l.exchange);
            let venue = colourize(&venue, "\x1b[2m", colour);
            format!("{price_amount}{venue}")
        }
        None => " ".repeat(35),
    }
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

fn colourize(text: &str, code: &str, colour: bool) -> String {
    if colour {
        format!("{code}{text}\x1b[0m")
    } else {
        text.to_string()
    }
}
