//! `rebro tui`: list, add, check, sync, default, remove — no commands to remember.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use rebro_core::{model::days_until, sync, Forge, Store};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Row, Table, TableState, Wrap};
use ratatui::{DefaultTerminal, Frame};

#[derive(PartialEq)]
enum Field {
    Forge,
    Host,
    Token,
}

enum Mode {
    List,
    Add { forge: usize, host: String, token: String, field: Field },
    ConfirmRemove(String),
    /// OAuth sign-in running on a background thread.
    SignIn { prompt: String, rx: mpsc::Receiver<SignInMsg>, cancel: Arc<AtomicBool> },
}

enum SignInMsg {
    Prompt(String),
    Done(Result<String, String>),
}

struct App {
    store: Store,
    table: TableState,
    mode: Mode,
    status: Line<'static>,
}

pub fn run(store: Store) -> anyhow::Result<()> {
    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableBracketedPaste)?;
    let mut app = App { store, table: TableState::default().with_selected(Some(0)), mode: Mode::List, status: help() };
    let res = app.run(&mut terminal);
    execute!(std::io::stdout(), DisableBracketedPaste)?;
    ratatui::restore();
    res
}

fn help() -> Line<'static> {
    Line::from(vec![
        "a".bold(), " add  ".into(), "i".bold(), " import  ".into(), "c".bold(), " check  ".into(), "s".bold(), " sync git/gh  ".into(),
        "d".bold(), " default  ".into(), "x".bold(), " remove  ".into(), "o".bold(), " token page  ".into(),
        "q".bold(), " quit".into(),
    ])
}

impl App {
    fn run(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        loop {
            self.pump_sign_in();
            terminal.draw(|f| self.draw(f))?;
            if !event::poll(Duration::from_millis(250))? {
                continue;
            }
            match event::read()? {
                Event::Paste(text) => self.paste(&text),
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
                        return Ok(());
                    }
                    if self.key(k.code, terminal)? {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
    }

    /// Start OAuth sign-in on a worker thread; the UI keeps drawing and shows the prompt.
    fn start_sign_in(&mut self, forge: Forge, host: String) {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        let label = format!("Contacting {host}…");
        std::thread::spawn(move || {
            let txp = tx.clone();
            let prompt = move |p: rebro_core::LoginPrompt| {
                let text = match &p {
                    rebro_core::LoginPrompt::DeviceCode { user_code, url, .. } => {
                        format!("Code {user_code} (copied). Approve at {url}, browser opened. Esc cancels.")
                    }
                    rebro_core::LoginPrompt::Browser { .. } => "Approve rebro in the browser that just opened. Esc cancels.".into(),
                };
                crate::show_prompt_quiet(p);
                let _ = txp.send(SignInMsg::Prompt(text));
            };
            let r = rebro_core::guarded(|| rebro_core::login(forge, &host, None, &prompt, &|| c2.load(Ordering::Relaxed)));
            let msg = match r {
                Ok(acct) => {
                    let steps = rebro_core::Store::load()
                        .ok()
                        .and_then(|s| std::env::current_exe().ok().map(|exe| sync::sync_all(&s, &exe)))
                        .unwrap_or_default();
                    let bad = steps.iter().filter(|s| !s.ok).count();
                    Ok(format!("signed in as {}{}", acct.key(), if bad == 0 { ", git/gh synced".into() } else { format!(", {bad} sync step(s) failed (press s)") }))
                }
                Err(e) => Err(format!("{e:#}")),
            };
            let _ = tx.send(SignInMsg::Done(msg));
        });
        self.mode = Mode::SignIn { prompt: label, rx, cancel };
    }

    fn pump_sign_in(&mut self) {
        let Mode::SignIn { prompt, rx, .. } = &mut self.mode else { return };
        loop {
            let msg = match rx.try_recv() {
                Ok(m) => m,
                Err(mpsc::TryRecvError::Empty) => return,
                // worker gone without Done: back to the list instead of a sign-in screen forever
                Err(mpsc::TryRecvError::Disconnected) => SignInMsg::Done(Err("the sign-in stopped unexpectedly; try again".into())),
            };
            match msg {
                SignInMsg::Prompt(p) => *prompt = p,
                SignInMsg::Done(r) => {
                    self.status = match r {
                        Ok(m) => Line::from(m).fg(Color::Green),
                        Err(e) => Line::from(format!("sign-in failed: {e}")).fg(Color::Red),
                    };
                    self.store = rebro_core::Store::load().unwrap_or(rebro_core::Store { path: self.store.path.clone(), accounts: vec![] });
                    self.mode = Mode::List;
                    return;
                }
            }
        }
    }

    fn selected_key(&self) -> Option<String> {
        self.table.selected().and_then(|i| self.store.accounts.get(i)).map(|a| a.key())
    }

    fn busy(&mut self, terminal: &mut DefaultTerminal, msg: &str) {
        self.status = Line::from(msg.to_string()).fg(Color::Yellow);
        let _ = terminal.draw(|f| self.draw(f));
    }

    /// Returns true to quit.
    fn key(&mut self, code: KeyCode, terminal: &mut DefaultTerminal) -> anyhow::Result<bool> {
        match &mut self.mode {
            Mode::List => match code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
                KeyCode::Down | KeyCode::Char('j') => self.table.select_next(),
                KeyCode::Up | KeyCode::Char('k') => self.table.select_previous(),
                KeyCode::Char('a') => {
                    self.mode = Mode::Add { forge: 0, host: Forge::GitHub.default_host().into(), token: String::new(), field: Field::Forge };
                    self.status = Line::from("←/→ forge  Enter: sign in with browser  (or Tab to Token and paste one)  Esc: cancel");
                }
                KeyCode::Char('i') => {
                    self.busy(terminal, "importing what gh, glab, tea and git already have…");
                    let outcomes = rebro_core::import::import_all(&mut self.store, false)?;
                    let changed = outcomes.iter().any(|o| matches!(o.result, rebro_core::import::Result_::Added | rebro_core::import::Result_::Updated));
                    if changed {
                        self.store = Store::load()?;
                        let _ = sync::sync_all(&self.store, &std::env::current_exe()?);
                    }
                    let failed = outcomes.iter().any(|o| o.result == rebro_core::import::Result_::Failed);
                    self.status = Line::from(rebro_core::import::summary(&outcomes)).fg(if failed { Color::Yellow } else { Color::Green });
                }
                KeyCode::Char('c') => {
                    self.busy(terminal, "checking every token…");
                    let accts = rebro_core::check_all(&mut self.store);
                    let bad = accts.iter().filter(|a| !a.health.as_ref().is_some_and(|h| h.ok)).count();
                    self.status = if bad == 0 {
                        Line::from(format!("all {} tokens valid", accts.len())).fg(Color::Green)
                    } else {
                        Line::from(format!("{bad} of {} tokens failed", accts.len())).fg(Color::Red)
                    };
                }
                KeyCode::Char('s') => {
                    self.busy(terminal, "wiring git, gh, glab…");
                    let steps = sync::sync_all(&self.store, &std::env::current_exe()?);
                    let failed: Vec<_> = steps.iter().filter(|s| !s.ok).map(|s| format!("{}: {}", s.what, s.detail)).collect();
                    self.status = if failed.is_empty() {
                        Line::from(format!("synced: {}", steps.iter().map(|s| s.what.clone()).collect::<Vec<_>>().join(", "))).fg(Color::Green)
                    } else {
                        Line::from(failed.join(" | ")).fg(Color::Red)
                    };
                }
                KeyCode::Char('d') => {
                    if let Some(key) = self.selected_key() {
                        rebro_core::set_default(&mut self.store, &key)?;
                        self.status = Line::from(format!("{key} is the default for its host (press s to hand it to gh)")).fg(Color::Green);
                    }
                }
                KeyCode::Char('x') | KeyCode::Delete => {
                    if let Some(key) = self.selected_key() {
                        self.status = Line::from(format!("remove {key} and its token? y/n")).fg(Color::Red);
                        self.mode = Mode::ConfirmRemove(key);
                    }
                }
                KeyCode::Char('o') => {
                    if let Some(a) = self.table.selected().and_then(|i| self.store.accounts.get(i)) {
                        let _ = crate::open_url(&a.forge.token_url(&a.host));
                    }
                }
                _ => {}
            },
            Mode::SignIn { cancel, .. } => {
                if code == KeyCode::Esc {
                    cancel.store(true, Ordering::Relaxed);
                    self.status = Line::from("cancelling sign-in…").fg(Color::Yellow);
                }
            }
            Mode::ConfirmRemove(key) => {
                let key = key.clone();
                if code == KeyCode::Char('y') {
                    rebro_core::remove_account(&mut self.store, &key)?;
                    self.status = Line::from(format!("removed {key}"));
                } else {
                    self.status = help();
                }
                self.mode = Mode::List;
            }
            Mode::Add { forge, host, token, field } => match code {
                KeyCode::Esc => {
                    self.mode = Mode::List;
                    self.status = help();
                }
                KeyCode::Tab | KeyCode::Down => {
                    *field = match field {
                        Field::Forge => Field::Host,
                        Field::Host => Field::Token,
                        Field::Token => Field::Forge,
                    }
                }
                KeyCode::BackTab | KeyCode::Up => {
                    *field = match field {
                        Field::Forge => Field::Token,
                        Field::Host => Field::Forge,
                        Field::Token => Field::Host,
                    }
                }
                KeyCode::Left | KeyCode::Right if *field == Field::Forge => {
                    let n = Forge::ALL.len();
                    *forge = if code == KeyCode::Left { (*forge + n - 1) % n } else { (*forge + 1) % n };
                    *host = Forge::ALL[*forge].default_host().into();
                }
                KeyCode::Char('o') if *field == Field::Forge => {
                    let _ = crate::open_url(&Forge::ALL[*forge].token_url(host));
                }
                KeyCode::Char(c) => match field {
                    Field::Host => host.push(c),
                    Field::Token => token.push(c),
                    Field::Forge => {}
                },
                KeyCode::Backspace => match field {
                    Field::Host => {
                        host.pop();
                    }
                    Field::Token => {
                        token.pop();
                    }
                    Field::Forge => {}
                },
                KeyCode::Enter if token.trim().is_empty() => {
                    let (f, h) = (Forge::ALL[*forge], host.clone());
                    self.start_sign_in(f, h);
                }
                KeyCode::Enter => {
                    let (f, h, t) = (Forge::ALL[*forge], host.clone(), token.clone());
                    self.busy(terminal, "verifying token with the forge…");
                    match rebro_core::add_account(&mut self.store, f, &h, &t, None) {
                        Ok(acct) => {
                            let steps = sync::sync_all(&self.store, &std::env::current_exe()?);
                            let bad = steps.iter().filter(|s| !s.ok).count();
                            let warn = acct.health.as_ref().map(|h| h.warnings.join("; ")).unwrap_or_default();
                            self.status = Line::from(format!(
                                "added {}{}{}",
                                acct.key(),
                                if bad == 0 { ", git/gh synced".to_string() } else { format!(", {bad} sync step(s) failed (press s)") },
                                if warn.is_empty() { String::new() } else { format!("  ⚠ {warn}") }
                            ))
                            .fg(if bad == 0 && warn.is_empty() { Color::Green } else { Color::Yellow });
                            self.mode = Mode::List;
                        }
                        Err(e) => self.status = Line::from(format!("not added: {e}")).fg(Color::Red),
                    }
                }
                _ => {}
            },
        }
        Ok(false)
    }

    fn paste(&mut self, text: &str) {
        if let Mode::Add { host, token, field, .. } = &mut self.mode {
            match field {
                Field::Host => host.push_str(text.trim()),
                Field::Token => token.push_str(text.trim()),
                Field::Forge => {}
            }
        }
    }

    fn draw(&mut self, f: &mut Frame) {
        let [title, body, status] = Layout::vertical([Constraint::Length(1), Constraint::Min(3), Constraint::Length(2)]).areas(f.area());
        f.render_widget(
            Line::from(vec![" rebro ".bold().reversed(), "  one place for GitHub · GitLab · Codeberg · Gitea".dim()]),
            title,
        );

        let rows = self.store.accounts.iter().map(|a| {
            let h = a.health.clone().unwrap_or_default();
            let (st, color) = match &a.health {
                None => ("?".to_string(), Color::DarkGray),
                Some(h) if !h.ok => ("FAILED".into(), Color::Red),
                Some(h) if !h.warnings.is_empty() => ("warn".into(), Color::Yellow),
                Some(_) => ("ok".into(), Color::Green),
            };
            let exp = h.expires.clone().unwrap_or_else(|| "-".into());
            let exp = match days_until(&exp) {
                Some(d) if d >= 0 => format!("{exp} ({d}d)"),
                _ => exp,
            };
            let notes = h.error.clone().into_iter().chain(h.warnings.clone()).collect::<Vec<_>>().join("; ");
            Row::new(vec![
                Span::raw(a.forge.short()),
                Span::raw(a.key()),
                Span::raw(if a.default { "★" } else { "" }),
                Span::styled(st, Style::default().fg(color)),
                Span::raw(exp),
                Span::raw(h.token_kind.unwrap_or_default()),
                Span::raw(notes).dim(),
            ])
        });
        let table = Table::new(
            rows,
            [
                Constraint::Length(8),
                Constraint::Length(32),
                Constraint::Length(3),
                Constraint::Length(7),
                Constraint::Length(19),
                Constraint::Length(17),
                Constraint::Min(10),
            ],
        )
        .header(Row::new(vec!["forge", "account", "def", "status", "expires", "kind", "notes"]).bold().underlined())
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .block(Block::bordered().title(format!(" accounts ({}) ", self.store.accounts.len())));
        f.render_stateful_widget(table, body, &mut self.table);
        if self.store.accounts.is_empty() {
            let inner = Rect { y: body.y + 2, height: 1, ..body.inner(ratatui::layout::Margin::new(2, 0)) };
            f.render_widget(Paragraph::new("No accounts yet: press a to add one (o opens the token page)").dim(), inner);
        }

        f.render_widget(Paragraph::new(self.status.clone()).wrap(Wrap { trim: true }), status);

        if let Mode::SignIn { prompt, .. } = &self.mode {
            sign_in_popup(f, prompt);
        }

        if let Mode::Add { forge, host, token, field } = &self.mode {
            let area = centered(f.area(), 70, 11);
            f.render_widget(Clear, area);
            let mark = |on: bool| if on { "▶ " } else { "  " };
            let fg = Forge::ALL[*forge];
            let lines = vec![
                Line::from(vec![mark(*field == Field::Forge).into(), "Forge  ".bold(), format!("◀ {} ▶", fg.name()).into()]),
                Line::from(vec![mark(*field == Field::Host).into(), "Host   ".bold(), host.clone().into()]),
                Line::from(vec![mark(*field == Field::Token).into(), "Token  ".bold(), "•".repeat(token.chars().count()).into(), "  (optional)".dim()]),
                Line::from(""),
                Line::from(vec!["Enter".bold(), " = sign in with your browser (OAuth). Nothing to create or paste.".into()]),
                Line::from("Advanced: Tab to Token and paste a token you made instead.").dim(),
                Line::from(format!("Token page (o on Forge): {}", fg.token_url(host))).dim(),
            ];
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(Block::bordered().title(" add account ")), area);
        }
    }
}

fn sign_in_popup(f: &mut Frame, prompt: &str) {
    let area = centered(f.area(), 74, 7);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(vec![Line::from(""), Line::from(prompt.to_string()).bold(), Line::from(""), Line::from("Waiting for approval…").dim()])
            .wrap(Wrap { trim: true })
            .block(Block::bordered().title(" sign in ")),
        area,
    );
}

fn centered(r: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(r.width);
    let h = h.min(r.height);
    Rect { x: r.x + (r.width - w) / 2, y: r.y + (r.height - h) / 2, width: w, height: h }
}
