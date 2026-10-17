//! Terminal map of the pond seen from above, with the listener at the bottom
//! looking up across the water, and a panel listing where each duck is.

use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::style::{Attribute, Color, Print, SetAttribute, SetBackgroundColor, SetForegroundColor};
use crossterm::{cursor, execute, queue, terminal};

use duck_pond::scene::{Command, Kind, Snapshot, Vec2, in_pond, on_bank};

/// Scene states waiting to be shown once their audio is heard.
pub type Timeline = Arc<Mutex<VecDeque<Snapshot>>>;

/// Area of the world drawn, in metres.
const VIEW_MIN: Vec2 = Vec2::new(-38.0, -6.0);
const VIEW_MAX: Vec2 = Vec2::new(38.0, 47.0);
const RINGS: [f64; 4] = [5.0, 10.0, 20.0, 40.0];
const PANEL_WIDTH: usize = 60;

#[derive(Clone, Copy, PartialEq)]
struct Style {
    fg: Color,
    bg: Color,
    bold: bool,
}

#[derive(Clone, Copy)]
struct Cell {
    ch: char,
    style: Style,
}

const WATER: Color = Color::DarkBlue;
const LAND: Color = Color::Reset;

fn kind_style(kind: Kind) -> (char, Color) {
    match kind {
        Kind::Hen => ('H', Color::Yellow),
        Kind::Drake => ('D', Color::Green),
        Kind::Duckling => ('d', Color::DarkYellow),
    }
}

/// Puts the terminal back however the visualizer exits.
struct Restore;

impl Drop for Restore {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), SetAttribute(Attribute::Reset), cursor::Show, terminal::LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

/// Draw the pond until the user presses q, Esc or Ctrl+C; p throws peas.
/// `played` counts the frames the audio device has taken, so the map matches
/// what is heard.
pub fn run(timeline: Timeline, played: Arc<AtomicU64>, commands: Sender<Command>) -> io::Result<()> {
    terminal::enable_raw_mode()?;
    let _restore = Restore;
    let mut out = io::stdout();
    execute!(out, terminal::EnterAlternateScreen, cursor::Hide)?;
    let mut current: Option<Snapshot> = None;
    let mut size = (0, 0);
    loop {
        if event::poll(Duration::from_millis(40))? {
            if let Event::Key(key) = event::read()? {
                let ctrl_c = key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        _ if ctrl_c => return Ok(()),
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Char('p') => {
                            let _ = commands.send(Command::ScatterPeas);
                        }
                        _ => {}
                    }
                }
            }
        }
        {
            let now = played.load(Ordering::Relaxed);
            let mut timeline = timeline.lock().unwrap();
            while timeline.front().is_some_and(|s| s.frame <= now) {
                current = timeline.pop_front();
            }
        }
        let new_size = terminal::size()?;
        if new_size != size {
            size = new_size;
            queue!(out, terminal::Clear(terminal::ClearType::All))?;
        }
        if let Some(snapshot) = &current {
            draw(&mut out, snapshot, size)?;
        }
    }
}

fn draw(out: &mut impl Write, snap: &Snapshot, (cols, rows): (u16, u16)) -> io::Result<()> {
    let (cols, rows) = (cols as usize, rows as usize);
    let panel = if cols >= PANEL_WIDTH + 40 { PANEL_WIDTH } else { 0 };
    let (w, h) = (cols - panel, rows.saturating_sub(1));
    if w == 0 || h == 0 {
        return Ok(());
    }

    // terminal cells are about twice as tall as they are wide
    let span = VIEW_MAX - VIEW_MIN;
    let mpc = (span.x / w as f64).max(span.y / (2.0 * h as f64));
    let mid = (VIEW_MIN + VIEW_MAX) * 0.5;
    let to_world = |col: usize, row: usize| {
        Vec2::new(
            mid.x + (col as f64 + 0.5 - w as f64 / 2.0) * mpc,
            mid.y - (row as f64 + 0.5 - h as f64 / 2.0) * 2.0 * mpc,
        )
    };
    let to_cell = |p: Vec2| {
        let col = ((p.x - mid.x) / mpc + w as f64 / 2.0).floor();
        let row = ((mid.y - p.y) / (2.0 * mpc) + h as f64 / 2.0).floor();
        ((0.0..w as f64).contains(&col) && (0.0..h as f64).contains(&row)).then_some((col as usize, row as usize))
    };

    let mut grid = vec![vec![Cell { ch: ' ', style: Style { fg: Color::Reset, bg: LAND, bold: false } }; w]; h];
    for (row, line) in grid.iter_mut().enumerate() {
        for (col, cell) in line.iter_mut().enumerate() {
            let p = to_world(col, row);
            let r = p.len();
            // does a distance ring pass through this cell?
            let reach = (p.x.abs() * mpc / 2.0 + p.y.abs() * mpc) / r.max(1e-9);
            let ring = RINGS.iter().any(|&ring| (r - ring).abs() <= reach);
            *cell = if in_pond(p) {
                let ripple = (col * 7 + row * 13) % 11 == 0;
                let ch = if ring { '·' } else if ripple { '~' } else { ' ' };
                Cell { ch, style: Style { fg: Color::Blue, bg: WATER, bold: false } }
            } else if on_bank(p) {
                Cell { ch: if ring { '·' } else { '.' }, style: Style { fg: Color::DarkYellow, bg: LAND, bold: false } }
            } else {
                let ch = if ring { '·' } else if (col * 5 + row * 3) % 13 == 0 { '"' } else { ' ' };
                Cell { ch, style: Style { fg: Color::DarkGreen, bg: LAND, bold: false } }
            };
            if ring {
                cell.style.fg = Color::DarkGrey;
            }
        }
    }
    let text = |grid: &mut Vec<Vec<Cell>>, (col, row): (usize, usize), s: &str, fg: Color, bold: bool| {
        for (k, ch) in s.chars().enumerate() {
            if let Some(cell) = grid[row].get_mut(col + k) {
                *cell = Cell { ch, style: Style { fg, bg: cell.style.bg, bold } };
            }
        }
    };
    for ring in RINGS {
        if let Some((col, row)) = to_cell(Vec2::new(0.0, ring)) {
            text(&mut grid, (col + 1, row), &format!("{ring}m"), Color::Grey, false);
        }
    }
    if let Some(cell) = to_cell(Vec2::default()) {
        text(&mut grid, cell, "@", Color::White, true);
    }
    for &pea in &snap.peas {
        if let Some((col, row)) = to_cell(pea) {
            text(&mut grid, (col, row), "•", Color::Green, true);
        }
    }
    // adults on top of ducklings, and both on top of peas
    let mut ducks: Vec<_> = snap.ducks.iter().collect();
    ducks.sort_by_key(|d| d.kind != Kind::Duckling);
    for duck in &ducks {
        let Some((col, row)) = to_cell(duck.pos) else { continue };
        let (ch, fg) = kind_style(duck.kind);
        let style = match duck.call {
            Some(_) => Style { fg: Color::White, bg: Color::Magenta, bold: true },
            None => Style { fg, bg: grid[row][col].style.bg, bold: true },
        };
        grid[row][col] = Cell { ch, style };
    }
    for duck in &ducks {
        if let (Some(call), Some((col, row))) = (duck.call, to_cell(duck.pos)) {
            if duck.kind != Kind::Duckling {
                text(&mut grid, (col + 2, row), call, Color::Magenta, true);
            }
        }
    }

    let panel_lines = if panel > 0 { panel_text(snap) } else { Vec::new() };
    let mut current: Option<Style> = None;
    for (row, line) in grid.iter().enumerate() {
        queue!(out, cursor::MoveTo(0, row as u16))?;
        for cell in line {
            set(out, cell.style, &mut current)?;
            queue!(out, Print(cell.ch))?;
        }
        if panel > 0 {
            let mut used = 0;
            for (s, style) in panel_lines.get(row).map(Vec::as_slice).unwrap_or(&[]) {
                let s: String = s.chars().take(panel - used).collect();
                used += s.chars().count();
                set(out, *style, &mut current)?;
                queue!(out, Print(s))?;
            }
            set(out, plain(Color::Reset), &mut current)?;
            queue!(out, Print(" ".repeat(panel - used)))?;
        }
    }
    let help = " p scatter peas · q quit · @ is you, facing up across the pond · rings at 5, 10, 20 and 40 m";
    let help: String = help.chars().chain(std::iter::repeat(' ')).take(cols).collect();
    set(out, plain(Color::DarkGrey), &mut current)?;
    queue!(out, cursor::MoveTo(0, h as u16), Print(help), SetAttribute(Attribute::Reset))?;
    out.flush()
}

/// Switch to `style` unless it is already active.
fn set(out: &mut impl Write, style: Style, current: &mut Option<Style>) -> io::Result<()> {
    if *current != Some(style) {
        queue!(
            out,
            SetAttribute(Attribute::Reset),
            SetForegroundColor(style.fg),
            SetBackgroundColor(style.bg),
            SetAttribute(if style.bold { Attribute::Bold } else { Attribute::NormalIntensity })
        )?;
        *current = Some(style);
    }
    Ok(())
}

fn plain(fg: Color) -> Style {
    Style { fg, bg: Color::Reset, bold: false }
}

/// One line per duck, nearest first: who, how far, which way, doing what.
fn panel_text(snap: &Snapshot) -> Vec<Vec<(String, Style)>> {
    let bold = |fg| Style { fg, bg: Color::Reset, bold: true };
    let mut lines = vec![
        vec![(" THE DUCK POND".to_string(), bold(Color::White))],
        vec![
            (" H".to_string(), bold(Color::Yellow)),
            (" hen  ".to_string(), plain(Color::Grey)),
            ("D".to_string(), bold(Color::Green)),
            (" drake  ".to_string(), plain(Color::Grey)),
            ("d".to_string(), bold(Color::DarkYellow)),
            (" duckling  ".to_string(), plain(Color::Grey)),
            ("•".to_string(), bold(Color::Green)),
            (format!(" {} peas", snap.peas.len()), plain(Color::Grey)),
        ],
        vec![],
    ];
    let mut ducks: Vec<_> = snap.ducks.iter().collect();
    ducks.sort_by(|a, b| a.pos.len().total_cmp(&b.pos.len()));
    for d in ducks {
        let (ch, fg) = kind_style(d.kind);
        let bearing = d.pos.x.atan2(d.pos.y).to_degrees();
        let side = if bearing.abs() < 5.0 {
            "ahead".to_string()
        } else {
            format!("{:3.0}° {}", bearing.abs(), if bearing < 0.0 { "L" } else { "R" })
        };
        let mut line = vec![
            (format!(" {ch} "), bold(fg)),
            (format!("{:<8}{:>5.1} m  {:<7} {:<15}", d.name, d.pos.len(), side, d.activity), plain(Color::Grey)),
        ];
        if let Some(call) = d.call {
            line.push((call.to_string(), bold(Color::Magenta)));
        }
        lines.push(line);
    }
    lines
}
