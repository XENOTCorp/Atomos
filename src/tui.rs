//! Operator TUI. Talks only through atoms and the control socket.
//! Never opens the HTTP listener. Criticality C1 (display) / C2 (CRUD).

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ratatui::backend::TestBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Tabs};
use ratatui::{Frame, Terminal};
use serde_json::{json, Value};

use crate::atom::AtomCtx;
use crate::config::Config;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Status,
    Json,
    Config,
    Control,
}

impl Tab {
    pub fn next(self) -> Self {
        match self {
            Tab::Status => Tab::Json,
            Tab::Json => Tab::Config,
            Tab::Config => Tab::Control,
            Tab::Control => Tab::Status,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Tab::Status => "Status",
            Tab::Json => "JSON",
            Tab::Config => "Config",
            Tab::Control => "Control",
        }
    }
}

/// `$HOME/atomos` → `target` (usually the ctl binary).
pub fn install_link(home: &Path, target: &Path) -> std::io::Result<PathBuf> {
    let link = home.join("atomos");
    if link.exists() || link.symlink_metadata().is_ok() {
        let _ = std::fs::remove_file(&link);
    }
    std::os::unix::fs::symlink(target, &link)?;
    Ok(link)
}

pub fn uds_cmd(sock: &Path, cmd: &str) -> Result<Value, String> {
    let mut s = UnixStream::connect(sock).map_err(|e| format!("server unreachable: {e}"))?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    let line = format!("{{\"cmd\":\"{cmd}\"}}\n");
    s.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    serde_json::from_slice(&buf).map_err(|e| e.to_string())
}

fn now_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn random_token() -> String {
    let mut b = [0u8; 12];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut b);
    }
    let mut out = String::with_capacity(24);
    for x in b {
        let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{x:02x}"));
    }
    out
}

/// Append `{"token","note","created_s"}` to `/keys` via `json.crud`.
pub fn add_key(ctx: &AtomCtx, keys_path: &Path, token: &str, note: &str) -> Result<Value, String> {
    ctx.run(
        "json.crud",
        json!({
            "path": keys_path.display().to_string(),
            "op": "add",
            "pointer": "/keys/-",
            "value": { "token": token, "note": note, "created_s": now_s() }
        }),
    )
    .map_err(|e| e.to_string())
}

pub fn del_key_at(ctx: &AtomCtx, keys_path: &Path, index: usize) -> Result<Value, String> {
    ctx.run(
        "json.crud",
        json!({
            "path": keys_path.display().to_string(),
            "op": "del",
            "pointer": format!("/keys/{index}")
        }),
    )
    .map_err(|e| e.to_string())
}

#[derive(Clone)]
pub struct JsonRow {
    pub label: String,
}

pub struct UiState {
    pub tab: Tab,
    pub rows: Vec<JsonRow>,
    pub selected: usize,
    pub reveal: bool,
    pub confirm: bool,
    pub last: String,
    pub bind: String,
    pub server: String,
    pub rss: String,
    pub json_path: PathBuf,
    pub sock: PathBuf,
    pub pretty: String,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            tab: Tab::Status,
            rows: Vec::new(),
            selected: 0,
            reveal: false,
            confirm: false,
            last: String::new(),
            bind: "127.0.0.1:8090".into(),
            server: "unknown".into(),
            rss: String::new(),
            json_path: PathBuf::from("data.json"),
            sock: PathBuf::from("/tmp/atomos.sock"),
            pretty: String::new(),
        }
    }
}

pub fn load_rows(path: &Path) -> (Vec<JsonRow>, String) {
    let raw = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return (Vec::new(), format!("read: {e}")),
    };
    let doc: Value = match serde_json::from_slice(&raw) {
        Ok(v) => v,
        Err(e) => return (Vec::new(), format!("json: {e}")),
    };
    let pretty = serde_json::to_string_pretty(&doc).unwrap_or_default();
    let rows = match doc.get("keys").and_then(|k| k.as_array()) {
        Some(arr) => arr
            .iter()
            .map(|v| {
                let label = if let Some(t) = v.get("token").and_then(|x| x.as_str()) {
                    let note = v.get("note").and_then(|x| x.as_str()).unwrap_or("");
                    format!("{t}  {note}")
                } else {
                    v.to_string()
                };
                JsonRow { label }
            })
            .collect(),
        None => Vec::new(),
    };
    (rows, pretty)
}

fn mask(s: &str) -> String {
    if s.len() <= 4 {
        "****".into()
    } else {
        format!("{}…{}", &s[..2], &s[s.len() - 2..])
    }
}

pub fn draw(f: &mut Frame, st: &UiState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(2),
        ])
        .split(f.area());
    let titles: Vec<Line> = [Tab::Status, Tab::Json, Tab::Config, Tab::Control]
        .into_iter()
        .map(|t| Line::from(Span::raw(t.label())))
        .collect();
    let tabs = Tabs::new(titles)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("ATOMOS")
                .border_style(Style::default().fg(Color::Gray)),
        )
        .style(Style::default().fg(Color::White).bg(Color::Black))
        .highlight_style(Style::default().fg(Color::White))
        .select(match st.tab {
            Tab::Status => 0,
            Tab::Json => 1,
            Tab::Config => 2,
            Tab::Control => 3,
        });
    f.render_widget(tabs, chunks[0]);
    match st.tab {
        Tab::Status => {
            let p = Paragraph::new(format!(
                "server {}\nbind {}\nrss {}\n{}\njson {}",
                st.server,
                st.bind,
                st.rss,
                st.last,
                st.json_path.display()
            ))
            .style(Style::default().fg(Color::Gray).bg(Color::Black))
            .block(Block::default().borders(Borders::ALL).title("status"));
            f.render_widget(p, chunks[1]);
        }
        Tab::Json => {
            if st.rows.is_empty() {
                let p = Paragraph::new(if st.pretty.is_empty() {
                    st.last.clone()
                } else {
                    st.pretty.clone()
                })
                .style(Style::default().fg(Color::White).bg(Color::Black))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("json  a add /keys/-  d del  v reveal"),
                );
                f.render_widget(p, chunks[1]);
            } else {
                let items: Vec<ListItem> = st
                    .rows
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        let text = if st.reveal {
                            r.label.clone()
                        } else {
                            mask(&r.label)
                        };
                        let mark = if i == st.selected { ">" } else { " " };
                        ListItem::new(format!("{mark} {text}"))
                    })
                    .collect();
                let list = List::new(items)
                    .style(Style::default().fg(Color::White).bg(Color::Black))
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title("keys  a add  d del  v reveal"),
                    );
                f.render_widget(list, chunks[1]);
            }
        }
        Tab::Config => {
            let p = Paragraph::new(format!(
                "bind {}   socket {}\nEdits: json.crud on the config file, then Control → f refresh-endpoints.\nMemory cap and TCP flags are in the same JSON.",
                st.bind,
                st.sock.display()
            ))
            .style(Style::default().fg(Color::Gray).bg(Color::Black))
            .block(Block::default().borders(Borders::ALL).title("config"));
            f.render_widget(p, chunks[1]);
        }
        Tab::Control => {
            let p = Paragraph::new(
                "r restart (confirm)  s stop  t start  f refresh-endpoints  b backup  y dry-test rules",
            )
            .style(Style::default().fg(Color::Gray).bg(Color::Black))
            .block(Block::default().borders(Borders::ALL).title("control"));
            f.render_widget(p, chunks[1]);
        }
    }
    let help = Paragraph::new("q quit  Tab pane  j/k list")
        .style(Style::default().fg(Color::DarkGray).bg(Color::Black));
    f.render_widget(help, chunks[2]);
}

pub fn render_test_buffer(st: &UiState) -> String {
    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).expect("term");
    term.draw(|f| draw(f, st)).expect("draw");
    let buf = term.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn refresh_json(ui: &mut UiState) {
    let (rows, pretty) = load_rows(&ui.json_path);
    ui.rows = rows;
    ui.pretty = pretty;
}

/// Interactive TUI. Separate process from the server.
pub fn run_tui(cfg: &Config, json_path: PathBuf) -> std::io::Result<()> {
    use crossterm::event::{self, Event, KeyCode};
    use crossterm::execute;
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
    use ratatui::backend::CrosstermBackend;

    let ctx = AtomCtx::test();
    let mut ui = UiState {
        bind: cfg.bind.clone(),
        sock: cfg.control_socket.clone(),
        json_path,
        ..UiState::default()
    };
    refresh_json(&mut ui);
    match uds_cmd(&cfg.control_socket, "status") {
        Ok(v) => {
            ui.server = v
                .get("state")
                .and_then(|s| s.as_str())
                .unwrap_or("on")
                .into();
        }
        Err(e) => {
            ui.server = "unreachable".into();
            ui.last = e;
        }
    }
    if let Ok(v) = ctx.run("resource.get", json!({})) {
        if let Some(n) = v.get("rss_bytes").and_then(|x| x.as_u64()) {
            ui.rss = format!("{n}");
        }
    }

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    loop {
        terminal.draw(|f| draw(f, &ui))?;
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        let Event::Key(k) = event::read()? else { continue };
        match k.code {
            KeyCode::Char('q') => break,
            KeyCode::Tab => ui.tab = ui.tab.next(),
            KeyCode::Char('j') => {
                if ui.selected + 1 < ui.rows.len() {
                    ui.selected += 1;
                }
            }
            KeyCode::Char('k') => ui.selected = ui.selected.saturating_sub(1),
            KeyCode::Char('v') => ui.reveal = !ui.reveal,
            KeyCode::Char('a') if ui.tab == Tab::Json => {
                let tok = random_token();
                match add_key(&ctx, &ui.json_path, &tok, "") {
                    Ok(_) => {
                        ui.last = format!("added {tok}");
                        refresh_json(&mut ui);
                    }
                    Err(e) => ui.last = e,
                }
            }
            KeyCode::Char('d') if ui.tab == Tab::Json => {
                if ui.confirm {
                    match del_key_at(&ctx, &ui.json_path, ui.selected) {
                        Ok(_) => {
                            ui.last = "deleted".into();
                            ui.confirm = false;
                            refresh_json(&mut ui);
                            ui.selected = ui.selected.min(ui.rows.len().saturating_sub(1));
                        }
                        Err(e) => ui.last = e,
                    }
                } else {
                    ui.confirm = true;
                    ui.last = "d again to confirm delete".into();
                }
            }
            KeyCode::Char('r') if ui.tab == Tab::Control => {
                if ui.confirm {
                    ui.last = match uds_cmd(&ui.sock, "restart") {
                        Ok(_) => "restart sent".into(),
                        Err(e) => e,
                    };
                    ui.confirm = false;
                } else {
                    ui.confirm = true;
                    ui.last = "r again to confirm restart".into();
                }
            }
            KeyCode::Char('s') if ui.tab == Tab::Control => {
                ui.last = match uds_cmd(&ui.sock, "stop") {
                    Ok(_) => "stop sent".into(),
                    Err(e) => e,
                };
            }
            KeyCode::Char('t') if ui.tab == Tab::Control => {
                ui.last = match uds_cmd(&ui.sock, "start") {
                    Ok(_) => "start sent".into(),
                    Err(e) => e,
                };
            }
            KeyCode::Char('f') if ui.tab == Tab::Control => {
                ui.last = match uds_cmd(&ui.sock, "refresh-endpoints") {
                    Ok(_) => "refresh sent".into(),
                    Err(e) => e,
                };
            }
            KeyCode::Char('y') if ui.tab == Tab::Control => {
                ui.last = match ctx.run("rules.dry_test", json!({})) {
                    Ok(v) => format!("{v}"),
                    Err(e) => e.to_string(),
                };
            }
            KeyCode::Char('b') if ui.tab == Tab::Control => {
                let dest = ui.json_path.with_extension("bak");
                ui.last = match ctx.run(
                    "settings.backup",
                    json!({
                        "path": ui.json_path.display().to_string(),
                        "dest": dest.display().to_string()
                    }),
                ) {
                    Ok(v) => format!("backup {v}"),
                    Err(e) => e.to_string(),
                };
            }
            _ => {}
        }
    }
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_contains_atomos() {
        let buf = render_test_buffer(&UiState::default());
        assert!(buf.contains("ATOMOS"), "{buf}");
    }

    #[test]
    fn install_link_temp_home() {
        let dir = tempfile::tempdir().unwrap();
        let tgt = dir.path().join("ctl-bin");
        std::fs::write(&tgt, b"x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tgt, std::fs::Permissions::from_mode(0o755));
        }
        let home = dir.path().join("home");
        std::fs::create_dir(&home).unwrap();
        let link = install_link(&home, &tgt).unwrap();
        let meta = std::fs::symlink_metadata(&link).unwrap();
        assert!(meta.file_type().is_symlink());
    }

    #[test]
    fn add_then_delete_key_row() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("data.json");
        std::fs::write(&p, r#"{"keys":[]}"#).unwrap();
        let ctx = AtomCtx::test();
        add_key(&ctx, &p, "ALPHA", "n").unwrap();
        let (rows, _) = load_rows(&p);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].label.contains("ALPHA"));
        del_key_at(&ctx, &p, 0).unwrap();
        let (rows, _) = load_rows(&p);
        assert!(rows.is_empty());
    }
}
