//! rust-crypto-orderbook client — a terminal viewer for the gRPC stream.
//!
//! Connects to `src/server.rs` and redraws the combined book in place, like
//! `top`. Not part of the service — just lets a reviewer see the merged
//! book without `grpcurl`.

use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use clap::Parser;
use rust_crypto_orderbook::exchange::Venue;
use rust_crypto_orderbook::orderbook::orderbook_aggregator_client::OrderbookAggregatorClient;
use rust_crypto_orderbook::orderbook::{Empty, Level, Summary};
use rust_crypto_orderbook::telemetry;
use tokio::time::sleep;
use tokio_stream::StreamExt;
use tracing::{info, warn};

/// Rows rendered per side. Fewer real levels leave blank rows so nothing
/// jumps between frames.
const ROWS: usize = 10;

/// All venues this project drives.
const VENUES: [Venue; 3] = [Venue::Binance, Venue::Bitstamp, Venue::Kraken];

#[derive(Parser)]
#[command(name = "client", version, about = "rust-crypto-orderbook demo client")]
struct Cli {
    /// gRPC server address, e.g. http://127.0.0.1:50051.
    #[arg(long, default_value = "http://127.0.0.1:50051")]
    addr: String,
}

/// Message count and rolling messages/second rate across frames.
struct Stats {
    count: u64,
    rate: f64,
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

    /// Records one message; recomputes the rolling rate if enough time has
    /// passed (guards against a near-zero elapsed interval producing NaN).
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

/// Client-side per-venue last-seen tracking for the `●`/`○ stale <Ns>`
/// header. Not the same as the server's staleness state — `Summary` has no
/// venue-health field, so this just tracks which venues appear per frame.
struct VenueTracker {
    last_seen: HashMap<Venue, Instant>,
}

impl VenueTracker {
    /// Seeds every venue with the current instant so the header prints
    /// "stale 0.0s" instead of a blank before the first frame.
    fn new(venues: &[Venue]) -> Self {
        let now = Instant::now();
        Self {
            last_seen: venues.iter().map(|v| (*v, now)).collect(),
        }
    }

    /// Records which venues appear in this frame's levels, bumps their
    /// last-seen instant, and returns that set.
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

    // Fixed 1s reconnect loop — simpler than the feed's exponential
    // backoff, covers both "server not up yet" and "server restarted."
    let reconnect_loop = async {
        loop {
            match connect_and_stream(&cli.addr, colour, &mut stats, &mut tracker).await {
                Ok(()) => info!("stream ended, reconnecting"),
                Err(e) => warn!(%e, "connect failed, retrying"),
            }
            sleep(Duration::from_secs(1)).await;
        }
    };

    // Explicit handler needed: as PID 1 in the container, Linux ignores an
    // unhandled SIGINT rather than terminating the process.
    tokio::select! {
        _ = reconnect_loop => {}
        _ = tokio::signal::ctrl_c() => {
            info!("received ctrl-c, exiting");
        }
    }

    Ok(())
}

/// Connects once, streams and renders `Summary` messages until the stream
/// ends or errors.
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

/// Table interior width in visible columns. `row()` pads every row to this
/// width so a redraw never needs a clear-to-end-of-line. A few columns
/// under 80 to leave room for `docker compose`'s log prefix.
const TABLE_WIDTH: usize = 72;

/// Builds one frame as a boxed table: cursor home, then borders and
/// fixed-width rows — never a full-screen clear, which would flicker.
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
    // Padded manually, not via format!'s width: the status string can
    // carry ANSI codes, which format! would count as raw bytes.
    let status = venue_status(venues, tracker, present, colour);
    let pad = 56usize.saturating_sub(visible_len(&status));
    row(
        &mut out,
        &format!("{status}{}{:>16}", " ".repeat(pad), now_hms()),
    );
    border(&mut out, '├', '┤');
    row(&mut out, &format!("{:^35} {:^35}", "BIDS", "ASKS"));
    border(&mut out, '├', '┤');

    // Shared max across both sides so a bid bar and ask bar of the same
    // length represent the same amount.
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

/// Wraps content in the table's side borders, padded to `TABLE_WIDTH` on
/// visible width (skipping ANSI codes) so a coloured cell doesn't misalign
/// the border.
fn row(out: &mut String, content: &str) {
    let pad = TABLE_WIDTH.saturating_sub(visible_len(content));
    out.push_str("│ ");
    out.push_str(content);
    out.push_str(&" ".repeat(pad));
    out.push_str(" │\n");
}

/// Visible character count, skipping ANSI escape sequences.
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

/// 256-colour depth-bar backgrounds, matching each side's foreground.
const BAR_BG_BID: &str = "48;5;22";
const BAR_BG_ASK: &str = "48;5;52";

/// Which edge a depth bar grows from — bids fill from the right, asks from
/// the left, so both grow toward the shared border between columns.
#[derive(Clone, Copy)]
enum FillFrom {
    Left,
    Right,
}

/// One bid or ask cell: price, amount, venue label, with a depth bar
/// proportional to `level`'s amount against `max_amount`. `None` renders
/// as blank padding so the layout doesn't reflow.
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

/// Wraps `plain` in ANSI codes, combining two boundaries: `CELL_FG_SPLIT`
/// (foreground switches to dim) and `fill_len` (background bar). Each run
/// restates both fg and bg explicitly, since SGR state persists otherwise.
fn shade_cell(
    plain: &str,
    price_fg: &str,
    bar_bg: &str,
    fill_len: usize,
    fill_from: FillFrom,
) -> String {
    let fill_len = fill_len.min(CELL_WIDTH);
    // Left shades [0, fill_len); Right shades the same length from the
    // opposite edge.
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
        // Guard against division by zero producing NaN/inf on screen.
        Some(m) if m != 0.0 => format!("{:.1} bps", summary.spread / m * 10_000.0),
        _ => "n/a".to_string(),
    };

    // Pad before colourizing so the visible width stays constant either way.
    let spread_field = format!("{:<40}", format!("spread {:>12.8} ({bps})", summary.spread));
    let spread_field = if summary.spread < 0.0 {
        colourize(&spread_field, "\x1b[1;31m", colour)
    } else {
        spread_field
    };

    let rate_text = if stats.rate.is_finite() {
        format!("{:.1}/s", stats.rate)
    } else {
        "-.-/s".to_string()
    };

    format!("{spread_field}{:>8} updates   {rate_text:<8}", stats.count)
}

/// Wall-clock HH:MM:SS, UTC.
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

/// Renders each venue's status: `binance ●` (green) if seen this frame,
/// `bitstamp ○ stale 4.2s` (red) otherwise.
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
