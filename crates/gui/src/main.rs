#![windows_subsystem = "windows"]
//! rebro desktop app. Layout, spacing and colours follow the Penpot design (file "rebro"):
//! Windows = Fluent (Settings-style nav + cards), macOS = sidebar + grouped list + sheets,
//! GNOME = libadwaita split view + boxed lists + header-bar dialogs. Follows light/dark live.

mod theme;

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{button::*, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rebro_core::{model::days_until, sync, Account, Forge, LoginPrompt, Store};
use theme::{c, Pal, Platform};

// ------------------------------------------------------------------ assets

const EMBEDDED: &[(&str, &[u8])] = &[
    ("icons/add.svg", include_bytes!("../assets/icons/add.svg")),
    ("icons/branch.svg", include_bytes!("../assets/icons/branch.svg")),
    ("icons/check.svg", include_bytes!("../assets/icons/check.svg")),
    ("icons/close.svg", include_bytes!("../assets/icons/close.svg")),
    ("icons/copy.svg", include_bytes!("../assets/icons/copy.svg")),
    ("icons/error.svg", include_bytes!("../assets/icons/error.svg")),
    ("icons/info.svg", include_bytes!("../assets/icons/info.svg")),
    ("icons/key.svg", include_bytes!("../assets/icons/key.svg")),
    ("icons/menu.svg", include_bytes!("../assets/icons/menu.svg")),
    ("icons/more.svg", include_bytes!("../assets/icons/more.svg")),
    ("icons/open.svg", include_bytes!("../assets/icons/open.svg")),
    ("icons/people.svg", include_bytes!("../assets/icons/people.svg")),
    ("icons/refresh.svg", include_bytes!("../assets/icons/refresh.svg")),
    ("icons/ring.svg", include_bytes!("../assets/icons/ring.svg")),
    ("icons/shield.svg", include_bytes!("../assets/icons/shield.svg")),
    ("icons/signin.svg", include_bytes!("../assets/icons/signin.svg")),
    ("icons/star.svg", include_bytes!("../assets/icons/star.svg")),
    ("icons/sync.svg", include_bytes!("../assets/icons/sync.svg")),
    ("icons/trash.svg", include_bytes!("../assets/icons/trash.svg")),
    ("icons/warn.svg", include_bytes!("../assets/icons/warn.svg")),
    ("logos/codeberg.svg", include_bytes!("../assets/logos/codeberg.svg")),
    ("logos/gitea.svg", include_bytes!("../assets/logos/gitea.svg")),
    ("logos/github.svg", include_bytes!("../assets/logos/github.svg")),
    ("logos/gitlab.svg", include_bytes!("../assets/logos/gitlab.svg")),
];

/// Our icons/logos under "rebro/", everything else from gpui-kit's default set.
struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if let Some(p) = path.strip_prefix("rebro/") {
            return Ok(EMBEDDED.iter().find(|(n, _)| *n == p).map(|(_, b)| Cow::Borrowed(*b)));
        }
        gpui_kit::assets::Assets.load(path)
    }
    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        gpui_kit::assets::Assets.list(path)
    }
}

fn icon(name: &str, size: f32, color: Hsla) -> Svg {
    svg().path(format!("rebro/icons/{name}.svg")).size(px(size)).flex_none().text_color(color)
}

fn logo(forge: Forge, size: f32, p: &Pal) -> Svg {
    let file = match forge {
        Forge::GitHub => "github",
        Forge::GitLab => "gitlab",
        Forge::Forgejo => "codeberg",
        Forge::Gitea => "gitea",
    };
    svg().path(format!("rebro/logos/{file}.svg")).size(px(size)).flex_none().text_color(p.logo_color(forge))
}

fn spinner(p: &Pal) -> impl IntoElement {
    svg()
        .path("rebro/icons/ring.svg")
        .size(px(18.))
        .flex_none()
        .text_color(p.accent)
        .with_animation("spin", Animation::new(Duration::from_millis(900)).repeat(), |s, d| s.with_transformation(Transformation::rotate(percentage(d))))
}

// ------------------------------------------------------------------ small widgets

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Accent,
    Standard,
    Subtle,
}

/// A native-looking push button for the current platform. Caller adds `.on_click`.
fn btn(id: impl Into<ElementId>, label: Option<&str>, ic: Option<&str>, kind: Kind, p: &Pal) -> Stateful<Div> {
    let (fill, fg, stroke) = match kind {
        Kind::Accent => (p.accent, p.on_accent, None),
        Kind::Standard if p.platform == Platform::Gnome => (p.flat, p.text, None),
        Kind::Standard => (p.control, p.text, Some(p.control_stroke)),
        Kind::Subtle => (transparent_black(), p.text, None),
    };
    let h = match p.platform {
        Platform::Windows => 32.,
        Platform::Mac => 26.,
        Platform::Gnome => 34.,
    };
    let hover = if kind == Kind::Accent { fill.opacity(0.9) } else { p.hover };
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .h(px(h))
        .when(label.is_some(), |d| d.px(px(12.)))
        .when(label.is_none(), |d| d.w(px(h)))
        .rounded(px(p.radius))
        .bg(fill)
        .when_some(stroke, |d, s| d.border_1().border_color(s))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .when_some(ic, |d, i| d.child(icon(i, 16., fg)))
        .when_some(label, |d, l| {
            d.child(div().text_color(fg).when(kind == Kind::Accent, |t| t.font_weight(FontWeight::SEMIBOLD)).child(l.to_string()))
        })
}

fn link(id: impl Into<ElementId>, label: impl Into<SharedString>, p: &Pal) -> Stateful<Div> {
    let accent = p.accent;
    div().id(id).text_color(accent).cursor_pointer().hover(|s| s.underline()).child(label.into())
}

fn open_url(url: &str) {
    if std::env::var("REBRO_NO_BROWSER").is_ok_and(|v| v == "1") {
        return;
    }
    let _ = webbrowser::open(url);
}

/// The git credential helper must be the CLI (rebro.exe), not this GUI binary.
fn cli_exe() -> std::path::PathBuf {
    std::env::current_exe()
        .map(|e| e.with_file_name(if cfg!(windows) { "rebro.exe" } else { "rebro" }))
        .unwrap_or_default()
}

// ------------------------------------------------------------------ state

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Accounts,
    Tools,
    About,
}

#[derive(Clone, Copy, PartialEq)]
enum Sev {
    Ok,
    Warn,
    Bad,
    Info,
}

struct Bar {
    sev: Sev,
    title: SharedString,
    msg: SharedString,
}

#[derive(Clone)]
enum Overlay {
    None,
    Add,
    Token,
    Code { forge: Forge, code: Option<SharedString>, url: SharedString },
    Browser { forge: Forge, url: Option<SharedString> },
    Remove { key: String },
}

enum SignInMsg {
    Prompt(LoginPrompt),
    Done(Result<String, String>),
}

struct Rebro {
    pal: Pal,
    page: Page,
    accounts: Vec<Account>,
    overlay: Overlay,
    add_forge: Forge,
    host: Entity<InputState>,
    token: Entity<InputState>,
    bar: Option<Bar>,
    busy: bool,
    cancel: Option<Arc<AtomicBool>>,
    tools: Option<sync::ToolsStatus>,
    _subs: Vec<Subscription>,
}

/// System light/dark, overridable with REBRO_THEME=light|dark (previews, screenshots).
fn is_dark(window: &Window) -> bool {
    match std::env::var("REBRO_THEME").unwrap_or_default().as_str() {
        "dark" => true,
        "light" => false,
        _ => matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark),
    }
}

fn apply_component_theme(dark: bool, window: &mut Window, cx: &mut App) {
    let mode = if dark { gpui_kit::component::ThemeMode::Dark } else { gpui_kit::component::ThemeMode::Light };
    gpui_kit::component::Theme::change(mode, Some(window), cx);
}

fn forge_name(f: Forge) -> &'static str {
    match f {
        Forge::GitHub => "GitHub",
        Forge::GitLab => "GitLab",
        Forge::Forgejo => "Codeberg",
        Forge::Gitea => "Gitea",
    }
}

/// One status line per account: (severity, text). No filler.
fn status_of(a: &Account) -> (Sev, String) {
    let Some(h) = &a.health else { return (Sev::Info, "Not checked yet".into()) };
    if !h.ok {
        let e = h.error.clone().unwrap_or_default();
        return (Sev::Bad, if e.contains("401") { "Sign-in revoked or expired".into() } else { e.chars().take(80).collect() });
    }
    let owners = if a.owners.is_empty() { String::new() } else { format!(" · used for {}", a.owners.iter().map(|o| format!("{o}/*")).collect::<Vec<_>>().join(", ")) };
    match h.expires.as_deref() {
        Some("never") => (Sev::Ok, format!("Signed in · never expires{owners}")),
        Some("auto-renewed") => (Sev::Ok, format!("Signed in · renews automatically{owners}")),
        Some(d) => match days_until(d) {
            Some(n) if n < 0 => (Sev::Bad, "Token expired".into()),
            Some(n) if n <= 14 => (Sev::Warn, format!("Token expires in {n} days")),
            Some(n) => (Sev::Ok, format!("Signed in · expires in {n} days{owners}")),
            None => (Sev::Ok, format!("Signed in{owners}")),
        },
        None => (Sev::Ok, format!("Signed in{owners}")),
    }
}

impl Rebro {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let platform = Platform::current();
        let dark = is_dark(window);
        let host = cx.new(|cx| InputState::new(window, cx).placeholder("github.com"));
        host.update(cx, |s, cx| s.set_value(Forge::GitHub.default_host(), window, cx));
        let token = cx.new(|cx| InputState::new(window, cx).placeholder("Paste the token").masked(true));
        // follow the system theme live (and keep gpui-component's menus/inputs in step)
        let sub = cx.observe_window_appearance(window, |this, window, cx| {
            this.pal = Pal::new(this.pal.platform, is_dark(window));
            apply_component_theme(is_dark(window), window, cx);
            cx.notify();
        });
        apply_component_theme(dark, window, cx);
        Self {
            pal: Pal::new(platform, dark),
            page: Page::Accounts,
            accounts: Store::load().map(|s| s.accounts).unwrap_or_default(),
            overlay: Overlay::None,
            add_forge: Forge::GitHub,
            host,
            token,
            bar: None,
            busy: false,
            cancel: None,
            tools: None,
            _subs: vec![sub],
        }
    }

    fn reload(&mut self) {
        self.accounts = Store::load().map(|s| s.accounts).unwrap_or_default();
    }

    fn show(&mut self, sev: Sev, title: impl Into<SharedString>, msg: impl Into<SharedString>) {
        self.bar = Some(Bar { sev, title: title.into(), msg: msg.into() });
    }

    fn open(&mut self, o: Overlay, cx: &mut Context<Self>) {
        self.overlay = o;
        cx.notify();
    }

    fn pick_forge(&mut self, f: Forge, window: &mut Window, cx: &mut Context<Self>) {
        self.add_forge = f;
        self.host.update(cx, |s, cx| s.set_value(f.default_host(), window, cx));
        cx.notify();
    }

    /// Blocking core work off the UI thread, then apply on the UI thread.
    fn background<R: Send + 'static>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        job: impl FnOnce() -> R + Send + 'static,
        done: impl FnOnce(&mut Self, R, &mut Window, &mut Context<Self>) + 'static,
    ) {
        self.busy = true;
        cx.notify();
        // a panic in core must not take the window down: it becomes an error bar
        let task = cx.background_executor().spawn(async move { rebro_core::catch_panic(job) });
        cx.spawn_in(window, async move |this, cx| {
            let r = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match r {
                    Ok(r) => done(this, r, window, cx),
                    Err(e) => this.show(Sev::Bad, "Something went wrong", e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.background(window, cx, || Store::load().map(|mut s| rebro_core::check_all(&mut s)).unwrap_or_default(), |this, accts, _, _| {
            this.accounts = accts;
            let bad = this.accounts.iter().filter(|a| a.health.as_ref().is_some_and(|h| !h.ok)).count();
            if bad == 0 {
                this.show(Sev::Ok, "All accounts work", "");
            } else {
                this.show(Sev::Bad, format!("{bad} account(s) need attention"), "");
            }
        });
    }

    /// Take over the credentials gh, glab, tea and git's helpers already hold: no sign-in needed.
    fn import_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.background(
            window,
            cx,
            || {
                let mut store = Store::load()?;
                let outcomes = rebro_core::import::import_all(&mut store, false)?;
                if outcomes.iter().any(|o| matches!(o.result, rebro_core::import::Result_::Added | rebro_core::import::Result_::Updated)) {
                    let _ = sync::sync_all(&Store::load()?, &cli_exe());
                }
                anyhow::Ok(outcomes)
            },
            |this, r, _, _| {
                this.reload();
                match r {
                    Ok(o) => {
                        let added: Vec<String> = o
                            .iter()
                            .filter(|x| matches!(x.result, rebro_core::import::Result_::Added | rebro_core::import::Result_::Updated))
                            .filter_map(|x| x.account.clone())
                            .collect();
                        let sev = if added.is_empty() { Sev::Info } else { Sev::Ok };
                        this.show(sev, rebro_core::import::summary(&o), added.join(", "));
                    }
                    Err(e) => this.show(Sev::Bad, "Import failed", format!("{e:#}")),
                }
            },
        );
    }

    fn sync_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.background(
            window,
            cx,
            || {
                let store = Store::load().ok();
                let steps = store.as_ref().map(|s| sync::sync_all(s, &cli_exe())).unwrap_or_default();
                let status = store.as_ref().map(|s| sync::tools_status(s, &cli_exe()));
                (steps.iter().filter(|s| !s.ok).map(|s| format!("{}: {}", s.what, s.detail)).collect::<Vec<_>>(), status)
            },
            |this, (failed, status), _, _| {
                this.tools = status;
                if failed.is_empty() { this.show(Sev::Ok, "In sync", "git and gh use your accounts") } else { this.show(Sev::Bad, "Sync failed", failed.join(" · ")) }
            },
        );
    }

    fn load_tools(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.background(window, cx, || Store::load().ok().map(|s| sync::tools_status(&s, &cli_exe())), |this, st, _, _| this.tools = st);
    }

    fn set_helper(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        if on {
            return self.sync_now(window, cx);
        }
        self.background(
            window,
            cx,
            || {
                let store = Store::load().ok();
                if let Some(s) = &store {
                    let mut hosts: Vec<&str> = s.accounts.iter().map(|a| a.host.as_str()).collect();
                    hosts.dedup();
                    for h in hosts {
                        let _ = sync::unset_git_helper(h);
                    }
                }
                store.map(|s| sync::tools_status(&s, &cli_exe()))
            },
            |this, st, _, _| {
                this.tools = st;
                this.show(Sev::Info, "Credential helper off", "git uses its own helper again");
            },
        );
    }

    /// OAuth sign-in: worker thread runs the flow, prompts come back over a channel.
    fn sign_in(&mut self, forge: Forge, host: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.cancel.is_some() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.overlay = match forge {
            Forge::GitHub | Forge::GitLab => Overlay::Code { forge, code: None, url: SharedString::default() },
            _ => Overlay::Browser { forge, url: None },
        };
        cx.notify();
        let (tx, rx) = mpsc::channel::<SignInMsg>();
        std::thread::spawn(move || {
            let txp = tx.clone();
            let prompt = move |p: LoginPrompt| {
                let _ = txp.send(SignInMsg::Prompt(p));
            };
            // guarded: a panic still ends with Done, so the dialog closes and Sign in works again
            let r = rebro_core::guarded(|| rebro_core::login(forge, &host, None, &prompt, &|| cancel.load(Ordering::Relaxed))).map_err(|e| format!("{e:#}"));
            let r = r.map(|acct| {
                if let Ok(s) = Store::load() {
                    let _ = sync::sync_all(&s, &cli_exe());
                }
                acct.username
            });
            let _ = tx.send(SignInMsg::Done(r));
        });
        let rx = Arc::new(Mutex::new(rx));
        cx.spawn_in(window, async move |this, cx| loop {
            let rx2 = rx.clone();
            let msg = cx.background_executor().spawn(async move { rx2.lock().ok().and_then(|r| r.recv().ok()) }).await;
            // worker gone without Done (should not happen): reset instead of leaving Sign in dead
            let Some(msg) = msg else {
                let _ = this.update_in(cx, |this, _, cx| {
                    if this.cancel.take().is_some() {
                        this.overlay = Overlay::None;
                        this.show(Sev::Bad, "Sign-in failed", "the sign-in stopped unexpectedly; try again");
                    }
                    cx.notify();
                });
                break;
            };
            let done = matches!(msg, SignInMsg::Done(_));
            let _ = this.update_in(cx, |this, _window, cx| {
                match msg {
                    SignInMsg::Prompt(LoginPrompt::DeviceCode { user_code, url, url_with_code }) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(user_code.clone()));
                        open_url(url_with_code.as_deref().unwrap_or(&url));
                        if let Overlay::Code { code, url: u, .. } = &mut this.overlay {
                            *code = Some(user_code.into());
                            *u = url.into();
                        }
                    }
                    SignInMsg::Prompt(LoginPrompt::Browser { url }) => {
                        open_url(&url);
                        if let Overlay::Browser { url: u, .. } = &mut this.overlay {
                            *u = Some(url.into());
                        }
                    }
                    SignInMsg::Done(r) => {
                        this.cancel = None;
                        this.overlay = Overlay::None;
                        this.reload();
                        match r {
                            Ok(user) => this.show(Sev::Ok, format!("Signed in as {user}"), "git and gh use it now"),
                            Err(e) if e == "cancelled" => {}
                            Err(e) => this.show(Sev::Bad, "Sign-in failed", e),
                        }
                    }
                }
                cx.notify();
            });
            if done {
                break;
            }
        })
        .detach();
    }

    fn cancel_overlay(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = &self.cancel {
            c.store(true, Ordering::Relaxed); // the worker reports back and closes the dialog
        } else {
            self.overlay = Overlay::None;
        }
        cx.notify();
    }

    fn add_token(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (forge, host) = (self.add_forge, self.host.read(cx).value().to_string());
        let token = self.token.read(cx).value().to_string();
        if token.trim().is_empty() {
            return;
        }
        self.background(
            window,
            cx,
            move || {
                Store::load().and_then(|mut s| {
                    let a = rebro_core::add_account(&mut s, forge, &host, &token, None)?;
                    let _ = sync::sync_all(&s, &cli_exe());
                    Ok(a.username)
                })
            },
            |this, r, window, cx| {
                this.reload();
                match r {
                    Ok(u) => {
                        this.overlay = Overlay::None;
                        this.token.update(cx, |s, cx| s.set_value("", window, cx));
                        this.show(Sev::Ok, format!("Signed in as {u}"), "git and gh use it now");
                    }
                    Err(e) => this.show(Sev::Bad, "Token not added", format!("{e:#}")),
                }
            },
        );
    }

    fn remove(&mut self, key: String, cx: &mut Context<Self>) {
        let r = Store::load().and_then(|mut s| rebro_core::remove_account(&mut s, &key));
        self.overlay = Overlay::None;
        self.reload();
        match r {
            Ok(()) => self.show(Sev::Info, format!("Removed {key}"), "its token is deleted from this device"),
            Err(e) => self.show(Sev::Bad, "Not removed", e.to_string()),
        }
        cx.notify();
    }

    fn make_default(&mut self, key: String, cx: &mut Context<Self>) {
        let r = Store::load().and_then(|mut s| {
            rebro_core::set_default(&mut s, &key)?;
            let _ = sync::sync_all(&s, &cli_exe());
            Ok(())
        });
        self.reload();
        match r {
            Ok(()) => self.show(Sev::Ok, format!("{key} is the default"), "gh uses it now"),
            Err(e) => self.show(Sev::Bad, "Not changed", e.to_string()),
        }
        cx.notify();
    }
}

// ------------------------------------------------------------------ views

impl Rebro {
    fn nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.pal;
        let item = |id: &'static str, label: &'static str, ic: &'static str, page: Page, cx: &mut Context<Self>| {
            let sel = self.page == page;
            let h = match p.platform {
                Platform::Windows => 36.,
                Platform::Mac => 28.,
                Platform::Gnome => 40.,
            };
            div()
                .id(id)
                .relative()
                .flex()
                .items_center()
                .gap(px(if p.platform == Platform::Windows { 14. } else { 10. }))
                .h(px(h))
                .px(px(if p.platform == Platform::Windows { 16. } else { 10. }))
                .rounded(px(if p.platform == Platform::Mac { 6. } else { p.radius.max(4.) }))
                .when(sel, |d| d.bg(p.nav_sel))
                .when(!sel, |d| d.hover(move |s| s.bg(p.hover)))
                .cursor_pointer()
                .when(sel && p.platform == Platform::Windows, |d| {
                    d.child(div().absolute().left(px(0.)).top(px((h - 16.) / 2.)).w(px(3.)).h(px(16.)).rounded(px(2.)).bg(p.accent))
                })
                .child(icon(ic, if p.platform == Platform::Mac { 16. } else { 18. }, if sel && p.platform == Platform::Mac { p.accent } else { p.text }))
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.page = page;
                    this.bar = None;
                    if page == Page::Tools {
                        this.load_tools(window, cx);
                    }
                    cx.notify();
                }))
        };
        let items = vec![
            item("nav-accounts", "Accounts", "people", Page::Accounts, cx).into_any_element(),
            item("nav-tools", "Git & tools", "branch", Page::Tools, cx).into_any_element(),
            item("nav-about", "About", "info", Page::About, cx).into_any_element(),
        ];
        match p.platform {
            Platform::Windows => div().flex().flex_col().gap(px(4.)).w(px(p.nav_w)).flex_none().h_full().p(px(8.)).children(items).into_any_element(),
            Platform::Mac => div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .w(px(p.nav_w))
                .flex_none()
                .h_full()
                .bg(p.side)
                .pt(px(52.)) // room for the traffic lights over the sidebar
                .px(px(10.))
                .children(items)
                .into_any_element(),
            Platform::Gnome => div()
                .flex()
                .flex_col()
                .w(px(p.nav_w))
                .flex_none()
                .h_full()
                .bg(p.side)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .h(px(46.))
                        .px(px(6.))
                        .child(btn("g-add", None, Some("add"), Kind::Subtle, &p).on_click(cx.listener(|this, _, _, cx| this.open(Overlay::Add, cx))))
                        .child(div().font_weight(FontWeight::BOLD).child("rebro"))
                        .child(btn("g-menu", None, Some("menu"), Kind::Subtle, &p)),
                )
                .child(div().flex().flex_col().gap(px(2.)).p(px(6.)).children(items))
                .into_any_element(),
        }
    }

    fn page_title(&self) -> &'static str {
        match self.page {
            Page::Accounts => "Accounts",
            Page::Tools => "Git & tools",
            Page::About => "About",
        }
    }

    /// Windows: big title + command buttons in the content. macOS: unified toolbar. GNOME: header bar.
    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.pal;
        let actions: Vec<AnyElement> = match self.page {
            Page::Accounts => vec![
                btn("check", (p.platform == Platform::Windows).then_some("Check"), Some("refresh"), if p.platform == Platform::Windows { Kind::Standard } else { Kind::Subtle }, &p)
                    .on_click(cx.listener(|this, _, w, cx| this.check(w, cx)))
                    .into_any_element(),
                // GNOME keeps "+" in the sidebar header only (as designed)
                btn("add", (p.platform == Platform::Windows).then_some("Add account"), Some("add"), if p.platform == Platform::Windows { Kind::Accent } else { Kind::Subtle }, &p)
                    .on_click(cx.listener(|this, _, _, cx| this.open(Overlay::Add, cx)))
                    .into_any_element(),
            ]
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !(p.platform == Platform::Gnome && *i == 1))
            .map(|(_, e)| e)
            .collect(),
            Page::Tools => vec![btn("sync", (p.platform == Platform::Windows).then_some("Sync now"), Some("sync"), if p.platform == Platform::Windows { Kind::Standard } else { Kind::Subtle }, &p)
                .on_click(cx.listener(|this, _, w, cx| this.sync_now(w, cx)))
                .into_any_element()],
            Page::About => vec![],
        };
        let busy = self.busy;
        match p.platform {
            Platform::Windows => div()
                .flex()
                .items_center()
                .justify_between()
                .h(px(48.))
                .mb(px(12.))
                .child(div().text_size(px(28.)).font_weight(FontWeight::SEMIBOLD).child(self.page_title()))
                .child(div().flex().items_center().gap(px(8.)).when(busy, |d| d.child(spinner(&p))).children(actions))
                .into_any_element(),
            Platform::Mac => div()
                .flex()
                .items_center()
                .justify_between()
                .h(px(52.))
                .pl(px(20.))
                .pr(px(16.))
                .bg(if p.dark { c(0x2A2A2A) } else { c(0xF6F6F6) })
                .border_b_1()
                .border_color(p.sep)
                .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(self.page_title()))
                .child(div().flex().items_center().gap(px(4.)).when(busy, |d| d.child(spinner(&p))).children(actions))
                .into_any_element(),
            Platform::Gnome => div()
                .flex()
                .items_center()
                .justify_between()
                .h(px(46.))
                .px(px(8.))
                .child(div().flex().gap(px(4.)).children(actions))
                .child(div().font_weight(FontWeight::BOLD).child(self.page_title()))
                .child(div().w(px(34.)).when(busy, |d| d.child(spinner(&p))))
                .into_any_element(),
        }
    }

    fn info_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.pal;
        let b = self.bar.as_ref()?;
        let (bg, fg, ic) = match b.sev {
            Sev::Ok => (p.ok_bg, p.ok, "check"),
            Sev::Warn => (p.warn_bg, p.warn, "warn"),
            Sev::Bad => (p.bad_bg, p.bad, "error"),
            Sev::Info => (p.info_bg, p.accent, "info"),
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .min_h(px(48.))
                .pl(px(16.))
                .pr(px(8.))
                .mb(px(8.))
                .rounded(px(p.card_radius.min(8.)))
                .bg(bg)
                .border_1()
                .border_color(p.stroke)
                .child(icon(ic, 18., fg))
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .flex_wrap()
                        .gap_x(px(12.))
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(b.title.clone()))
                        .child(div().child(b.msg.clone())),
                )
                .child(btn("bar-close", None, Some("close"), Kind::Subtle, &p).on_click(cx.listener(|this, _, _, cx| {
                    this.bar = None;
                    cx.notify();
                })))
                .into_any_element(),
        )
    }

    fn more_menu(&self, i: usize, a: &Account, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let (key, host, user, forge, is_default) = (a.key(), a.host.clone(), a.git_username().to_string(), a.forge, a.default);
        let p = self.pal;
        Button::new(("more", i))
            .ghost()
            .small()
            .icon(Icon::empty().path("rebro/icons/more.svg").text_color(p.text))
            .dropdown_menu(move |menu, _, _| {
                let (t1, t2, t3) = (this.clone(), this.clone(), this.clone());
                let (k1, k3) = (key.clone(), key.clone());
                let (h2, u, h4) = (host.clone(), user.clone(), host.clone());
                let profile = format!("https://{h4}/{}", key.rsplit('/').next().unwrap_or_default());
                menu.when(!is_default, |m| {
                    m.item(PopupMenuItem::new("Make default").icon(Icon::empty().path("rebro/icons/star.svg")).on_click(move |_, _, app| {
                        let k = k1.clone();
                        let _ = t1.update(app, |this, cx| this.make_default(k, cx));
                    }))
                })
                .item(PopupMenuItem::new("Sign in again").icon(Icon::empty().path("rebro/icons/signin.svg")).on_click(move |_, window, app| {
                    let h = h2.clone();
                    let _ = t2.update(app, |this, cx| this.sign_in(forge, h, window, cx));
                }))
                .item(PopupMenuItem::new("Copy username").icon(Icon::empty().path("rebro/icons/copy.svg")).on_click(move |_, _, app| {
                    app.write_to_clipboard(ClipboardItem::new_string(u.clone()));
                }))
                .item(PopupMenuItem::new(format!("Open on {h4}")).icon(Icon::empty().path("rebro/icons/open.svg")).on_click(move |_, _, _| open_url(&profile)))
                .separator()
                .item(PopupMenuItem::new("Remove").icon(Icon::empty().path("rebro/icons/trash.svg")).on_click(move |_, _, app| {
                    let k = k3.clone();
                    let _ = t3.update(app, |this, cx| this.open(Overlay::Remove { key: k }, cx));
                }))
            })
    }

    fn account_row(&self, i: usize, a: &Account, cx: &mut Context<Self>) -> AnyElement {
        let p = self.pal;
        let (sev, detail) = status_of(a);
        let status_color = match sev {
            Sev::Ok | Sev::Info => p.text2,
            Sev::Warn => p.warn,
            Sev::Bad => p.bad,
        };
        let dot = match sev {
            Sev::Ok => p.ok,
            Sev::Warn => p.warn,
            Sev::Bad => p.bad,
            Sev::Info => p.text3,
        };
        let (forge, host) = (a.forge, a.host.clone());
        let windows = p.platform == Platform::Windows;
        let title = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(a.key())
            .when(a.default && windows, |d| {
                d.child(div().px(px(6.)).h(px(20.)).flex().items_center().rounded(px(4.)).bg(p.layer).border_1().border_color(p.stroke).text_size(px(p.small_size)).text_color(p.text2).child("Default"))
            })
            .when(a.default && !windows, |d| d.child(div().text_color(p.text2).child("· Default")));
        let status = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(p.small_size))
            .text_color(status_color)
            .when(windows, |d| d.child(div().size(px(8.)).rounded_full().bg(dot)))
            .child(detail);
        div()
            .flex()
            .items_center()
            .gap(px(if windows { 16. } else { 12. }))
            .h(px(p.row_h))
            .px(px(if windows { 16. } else { 14. }))
            .pr(px(8.))
            .when(windows, |d| d.rounded(px(p.card_radius)).bg(p.card).border_1().border_color(p.stroke))
            .child(logo(a.forge, match p.platform { Platform::Gnome => 32., _ => 24. }, &p))
            .child(div().flex().flex_col().gap(px(2.)).flex_1().min_w_0().child(title).child(status))
            .when(sev == Sev::Bad, |d| {
                d.child(btn(("again", i), Some(if windows { "Sign in again" } else { "Sign In Again" }), None, Kind::Standard, &p).on_click(cx.listener(move |this, _, w, cx| this.sign_in(forge, host.clone(), w, cx))))
            })
            .child(self.more_menu(i, a, cx))
            .into_any_element()
    }

    fn accounts_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.pal;
        if self.accounts.is_empty() {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .items_center()
                .justify_center()
                .gap(px(16.))
                .child(div().size(px(72.)).rounded_full().bg(p.nav_sel).flex().items_center().justify_center().child(icon("people", 36., p.text2)))
                .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).child("No accounts yet"))
                .child(btn("empty-add", Some("Add account"), Some("add"), Kind::Accent, &p).on_click(cx.listener(|this, _, _, cx| this.open(Overlay::Add, cx))))
                .child(
                    link("empty-import", if p.platform == Platform::Windows { "Import from this computer" } else { "Import from This Computer" }, &p)
                        .on_click(cx.listener(|this, _, w, cx| this.import_now(w, cx))),
                )
                .into_any_element();
        }
        let rows: Vec<AnyElement> = self.accounts.clone().iter().enumerate().map(|(i, a)| self.account_row(i, a, cx)).collect();
        match p.platform {
            Platform::Windows => div().id("list").flex().flex_col().gap(px(4.)).overflow_y_scroll().children(rows).into_any_element(),
            _ => {
                let mut list = div().flex().flex_col().rounded(px(p.card_radius)).bg(p.card).border_1().border_color(p.stroke).overflow_hidden();
                for (i, r) in rows.into_iter().enumerate() {
                    if i > 0 {
                        list = list.child(div().h(px(1.)).bg(p.sep).when(p.platform == Platform::Mac, |d| d.ml(px(52.))));
                    }
                    list = list.child(r);
                }
                div().id("list").overflow_y_scroll().child(list).into_any_element()
            }
        }
    }

    fn setting_card(&self, ic: &str, title: impl Into<SharedString>, desc: impl Into<SharedString>, right: AnyElement) -> AnyElement {
        let p = self.pal;
        div()
            .flex()
            .items_center()
            .gap(px(16.))
            .h(px(68.))
            .px(px(16.))
            .rounded(px(p.card_radius))
            .bg(p.card)
            .border_1()
            .border_color(p.stroke)
            .child(icon(ic, 20., p.text))
            .child(div().flex().flex_col().gap(px(2.)).flex_1().child(title.into()).child(div().text_size(px(p.small_size)).text_color(p.text2).child(desc.into())))
            .child(right)
            .into_any_element()
    }

    fn tools_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.pal;
        let Some(st) = &self.tools else {
            return div().flex().items_center().gap(px(10.)).child(spinner(&p)).child(div().text_color(p.text2).child("Checking…")).into_any_element();
        };
        let on = !st.helper.is_empty() && st.helper.iter().all(|(_, on)| *on);
        let hosts = if st.helper.is_empty() { "No accounts yet".to_string() } else { st.helper.iter().map(|(h, _)| h.as_str()).collect::<Vec<_>>().join(" · ") };
        let toggle = div()
            .id("helper-toggle")
            .flex()
            .items_center()
            .gap(px(12.))
            .cursor_pointer()
            .child(if on { "On" } else { "Off" })
            .child(
                div()
                    .w(px(40.))
                    .h(px(20.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .px(px(4.))
                    .when(on, |d| d.bg(p.accent).justify_end())
                    .when(!on, |d| d.border_1().border_color(p.text2).justify_start())
                    .child(div().size(px(if on { 12. } else { 10. })).rounded_full().bg(if on { p.on_accent } else { p.text2 })),
            )
            .on_click(cx.listener(move |this, _, w, cx| this.set_helper(!on, w, cx)));
        let gh = match &st.gh {
            None => link("get-gh", "Get gh", &p).on_click(|_, _, _| open_url("https://cli.github.com/")).into_any_element(),
            Some(None) => div().text_color(p.warn).child("Not signed in · Sync now fixes it").into_any_element(),
            Some(Some(u)) => div().flex().items_center().gap(px(6.)).child(icon("check", 16., p.ok)).child(div().text_color(p.text2).child(format!("In sync · {u}"))).into_any_element(),
        };
        let glab = if st.glab_installed {
            div().text_color(p.text2).child("Installed").into_any_element()
        } else {
            link("get-glab", "Get glab", &p).on_click(|_, _, _| open_url("https://gitlab.com/gitlab-org/cli")).into_any_element()
        };
        let storage = if cfg!(windows) { "Windows Credential Manager" } else if cfg!(target_os = "macos") { "macOS Keychain" } else { "Secret Service (GNOME Keyring)" };
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(self.setting_card("key", "Git credential helper", hosts, toggle.into_any_element()))
            .child(self.setting_card("branch", "GitHub CLI (gh)", "Uses your default github.com account", gh))
            .child(self.setting_card("branch", "GitLab CLI (glab)", "Uses your default GitLab token", glab))
            .child(self.setting_card("shield", "Credential storage", storage, div().into_any_element()))
            .into_any_element()
    }

    fn about_view(&self) -> AnyElement {
        let p = self.pal;
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(self.setting_card("info", format!("rebro {}", env!("CARGO_PKG_VERSION")), "Accounts for GitHub, GitLab, Codeberg and Gitea", div().into_any_element()))
            .child(self.setting_card(
                "open",
                "Source",
                "github.com/TuanKietTran/rebro",
                link("about-src", "Open", &p).on_click(|_, _, _| open_url("https://github.com/TuanKietTran/rebro")).into_any_element(),
            ))
            .into_any_element()
    }

    // ---------------------------------------------------------------- dialogs

    fn forge_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.pal;
        let cur = self.add_forge;
        let forges = [(Forge::GitHub, "GitHub"), (Forge::GitLab, "GitLab"), (Forge::Forgejo, "Codeberg"), (Forge::Gitea, "Gitea / Forgejo")];
        match p.platform {
            Platform::Windows => div()
                .w_full()
                .flex()
                .flex_wrap()
                .gap(px(8.))
                .children(forges.iter().enumerate().map(|(i, (f, n))| {
                    let (f, sel) = (*f, *f == cur);
                    div()
                        .id(("tile", i))
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .w(px(210.)) // 2 × 210 + 8 gap fits the 432 body without sub-pixel wrap
                        .h(px(56.))
                        .px(px(16.))
                        .rounded(px(4.))
                        .bg(p.card)
                        .border_color(if sel { p.accent } else { p.stroke })
                        .when(sel, |d| d.border_2())
                        .when(!sel, |d| d.border_1().hover(move |s| s.bg(p.hover)))
                        .cursor_pointer()
                        .child(logo(f, 24., &p))
                        .child(div().when(sel, |d| d.font_weight(FontWeight::SEMIBOLD)).child(*n))
                        .on_click(cx.listener(move |this, _, w, cx| this.pick_forge(f, w, cx)))
                }))
                .into_any_element(),
            Platform::Mac => div()
                .flex()
                .p(px(2.))
                .rounded(px(7.))
                .bg(p.flat)
                .children(forges.iter().enumerate().map(|(i, (f, n))| {
                    let (f, sel) = (*f, *f == cur);
                    div()
                        .id(("seg", i))
                        .flex_1()
                        .flex()
                        .justify_center()
                        .h(px(24.))
                        .items_center()
                        .rounded(px(6.))
                        .when(sel, |d| d.bg(p.card).font_weight(FontWeight::SEMIBOLD))
                        .cursor_pointer()
                        .child(n.split(' ').next().unwrap_or(n).to_string())
                        .on_click(cx.listener(move |this, _, w, cx| this.pick_forge(f, w, cx)))
                }))
                .into_any_element(),
            Platform::Gnome => {
                let mut list = div().flex().flex_col().rounded(px(12.)).bg(p.card).border_1().border_color(p.stroke).overflow_hidden();
                for (i, (f, n)) in forges.iter().enumerate() {
                    let (f, sel) = (*f, *f == cur);
                    if i > 0 {
                        list = list.child(div().h(px(1.)).bg(p.sep));
                    }
                    list = list.child(
                        div()
                            .id(("radio", i))
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .h(px(50.))
                            .px(px(14.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(p.hover))
                            .child(
                                div()
                                    .size(px(18.))
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(sel, |d| d.bg(p.accent).child(div().size(px(6.)).rounded_full().bg(c(0xFFFFFF))))
                                    .when(!sel, |d| d.border_1().border_color(p.text3)),
                            )
                            .child(logo(f, 22., &p))
                            .child(*n)
                            .on_click(cx.listener(move |this, _, w, cx| this.pick_forge(f, w, cx))),
                    );
                }
                list.into_any_element()
            }
        }
    }

    fn field(&self, label: &'static str, input: &Entity<InputState>) -> AnyElement {
        let p = self.pal;
        match p.platform {
            Platform::Mac => div().flex().items_center().gap(px(10.)).child(div().text_color(p.text2).child(format!("{label}:"))).child(div().flex_1().child(Input::new(input).small())).into_any_element(),
            _ => div().flex().flex_col().gap(px(8.)).child(label).child(Input::new(input)).into_any_element(),
        }
    }

    /// Platform dialog chrome around `body`, with primary/secondary actions.
    fn dialog(
        &self,
        title: impl Into<SharedString>,
        body: AnyElement,
        primary: (&'static str, Box<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)>),
        secondary: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.pal;
        let title: SharedString = title.into();
        let (plabel, pact) = primary;
        let primary_btn = btn("dlg-primary", Some(plabel), None, Kind::Accent, &p).on_click(cx.listener(move |this, _, w, cx| pact(this, w, cx)));
        let cancel_btn = btn("dlg-cancel", Some("Cancel"), None, if p.platform == Platform::Gnome { Kind::Subtle } else { Kind::Standard }, &p)
            .on_click(cx.listener(|this, _, _, cx| this.cancel_overlay(cx)));
        let panel = match p.platform {
            Platform::Windows => div()
                .w(px(480.))
                .flex()
                .flex_col()
                .rounded(px(8.))
                .bg(p.layer)
                .border_1()
                .border_color(if p.dark { c(0x3A3A3A) } else { c(0xD6D6D6) })
                .shadow_xl()
                .overflow_hidden()
                .child(div().flex().flex_col().gap(px(16.)).p(px(24.)).child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).child(title)).child(body).when_some(secondary, |d, s| d.child(s)))
                .child(div().h(px(1.)).bg(p.stroke))
                .child(div().flex().gap(px(8.)).p(px(24.)).bg(p.bg).child(primary_btn.flex_1()).child(cancel_btn.flex_1()))
                .into_any_element(),
            Platform::Mac => div()
                .w(px(420.))
                .flex()
                .flex_col()
                .gap(px(14.))
                .p(px(20.))
                .rounded(px(12.))
                .bg(if p.dark { c(0x2C2C2E) } else { c(0xFBFBFD) })
                .border_1()
                .border_color(p.control_stroke)
                .shadow_xl()
                .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                .child(body)
                .child(div().flex().items_center().gap(px(8.)).child(div().flex_1().when_some(secondary, |d, s| d.child(s))).child(cancel_btn).child(primary_btn))
                .into_any_element(),
            Platform::Gnome => div()
                .w(px(420.))
                .flex()
                .flex_col()
                .rounded(px(14.))
                .bg(p.bg)
                .border_1()
                .border_color(p.control_stroke)
                .shadow_xl()
                .overflow_hidden()
                .child(div().flex().items_center().justify_between().h(px(46.)).px(px(8.)).child(cancel_btn).child(div().font_weight(FontWeight::BOLD).child(title)).child(primary_btn))
                .child(div().flex().flex_col().gap(px(18.)).pt(px(12.)).px(px(18.)).pb(px(24.)).child(body).when_some(secondary, |d, s| d.child(s)))
                .into_any_element(),
        };
        // scrim; macOS sheets hang from the toolbar instead of centring
        div()
            .id("scrim")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .when(p.platform == Platform::Mac, |d| d.items_start().pt(px(52.)))
            .when(p.platform != Platform::Mac, |d| d.items_center())
            .bg(black().opacity(if p.platform == Platform::Mac { 0.12 } else { 0.3 }))
            .occlude() // modal: nothing behind the dialog receives the mouse
            .child(div().id("panel").on_click(|_, _, _| {}).child(panel))
            .into_any_element()
    }

    fn overlay_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.pal;
        let mac = p.platform == Platform::Mac;
        Some(match self.overlay.clone() {
            Overlay::None => return None,
            Overlay::Add => {
                let body = div().flex().flex_col().gap(px(16.)).child(self.forge_picker(cx)).child(self.field("Host", &self.host)).into_any_element();
                let token_link = link("use-token", if mac { "Use a Token…" } else if p.platform == Platform::Gnome { "Use a Token Instead" } else { "Use a token instead" }, &p)
                    .on_click(cx.listener(|this, _, _, cx| this.open(Overlay::Token, cx)));
                let import_link = link("dlg-import", if mac || p.platform == Platform::Gnome { "Import from This Computer" } else { "Import from this computer" }, &p)
                    .on_click(cx.listener(|this, _, w, cx| {
                        this.cancel_overlay(cx);
                        this.import_now(w, cx);
                    }));
                let token_link = div().flex().flex_col().gap(px(6.)).child(token_link).child(import_link).into_any_element();
                self.dialog(
                    if mac || p.platform == Platform::Gnome { "Add Account" } else { "Add account" },
                    body,
                    (if mac || p.platform == Platform::Gnome { "Sign In" } else { "Sign in" }, Box::new(|this, w, cx| {
                        let (f, h) = (this.add_forge, this.host.read(cx).value().to_string());
                        this.sign_in(f, h, w, cx);
                    })),
                    Some(token_link),
                    cx,
                )
            }
            Overlay::Code { forge, code, url } => {
                let body = div()
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .child(match code.clone() {
                        Some(code) => div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .child(div().text_size(px(32.)).font_weight(FontWeight::SEMIBOLD).font_family(p.mono).child(code))
                            .child(div().flex().items_center().gap(px(6.)).h(px(24.)).pl(px(6.)).pr(px(8.)).rounded_full().bg(p.ok_bg).text_color(p.ok).text_size(px(12.)).child(icon("check", 14., p.ok)).child("Copied"))
                            .into_any_element(),
                        None => div().text_color(p.text2).child(format!("Contacting {}…", forge_name(forge))).into_any_element(),
                    })
                    .child(div().flex().items_center().gap(px(10.)).child(spinner(&p)).child(div().text_color(p.text2).child(if url.is_empty() { "Waiting for approval".to_string() } else { format!("Waiting for approval in {}", url.trim_start_matches("https://")) })))
                    .into_any_element();
                let u = url.to_string();
                self.dialog(format!("Sign in to {}", forge_name(forge)), body, (if forge == Forge::GitHub { "Open GitHub" } else { "Open GitLab" }, Box::new(move |_, _, _| open_url(&u))), None, cx)
            }
            Overlay::Browser { forge, url } => {
                let body = div().flex().items_center().gap(px(10.)).child(spinner(&p)).child(div().text_color(p.text2).child("Approve rebro in your browser")).into_any_element();
                let u = url.map(|s| s.to_string()).unwrap_or_default();
                self.dialog(format!("Sign in to {}", forge_name(forge)), body, ("Open browser again", Box::new(move |_, _, _| open_url(&u))), None, cx)
            }
            Overlay::Token => {
                let host = self.host.read(cx).value().to_string();
                let url = self.add_forge.token_url(&host);
                let body = div().flex().flex_col().gap(px(16.)).child(self.field("Host", &self.host)).child(self.field("Token", &self.token)).into_any_element();
                let create = link("create-token", format!("Create a token on {host}"), &p).on_click(move |_, _, _| open_url(&url)).into_any_element();
                self.dialog(if mac { "Use a Token" } else { "Use a token" }, body, ("Add", Box::new(|this, w, cx| this.add_token(w, cx))), Some(create), cx)
            }
            Overlay::Remove { key } => {
                let k = key.clone();
                let body = div().child("Its token is deleted from this device.").into_any_element();
                self.dialog(format!("Remove {key}?"), body, ("Remove", Box::new(move |this, _, cx| this.remove(k.clone(), cx))), None, cx)
            }
        })
    }
}

impl Render for Rebro {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.pal;
        let body = match self.page {
            Page::Accounts => self.accounts_view(cx),
            Page::Tools => self.tools_view(cx),
            Page::About => self.about_view(),
        };
        let bar = self.info_bar(cx);
        let content_inner = div().flex().flex_col().flex_1().min_h_0().children(bar).child(body);
        let main = match p.platform {
            Platform::Windows => div()
                .flex()
                .flex_col()
                .flex_1()
                .h_full()
                .bg(p.layer)
                .border_l_1()
                .border_t_1()
                .border_color(p.stroke)
                .rounded_tl(px(8.))
                .pt(px(20.))
                .pl(px(20.))
                .pr(px(36.))
                .pb(px(24.))
                .child(self.header(cx))
                .child(content_inner),
            Platform::Mac => div().flex().flex_col().flex_1().h_full().bg(p.layer).child(self.header(cx)).child(div().flex().flex_col().flex_1().min_h_0().p(px(20.)).px(px(24.)).child(content_inner)),
            Platform::Gnome => div().flex().flex_col().flex_1().h_full().bg(p.layer).child(self.header(cx)).child(div().flex().flex_col().flex_1().min_h_0().py(px(24.)).px(px(48.)).child(content_inner)),
        };
        div()
            .relative()
            .size_full()
            .flex()
            .bg(p.bg)
            .font_family(p.font)
            .text_size(px(p.text_size))
            .text_color(p.text)
            .child(self.nav(cx))
            .child(main)
            .children(self.overlay_view(cx))
    }
}

fn main() {
    rebro_core::extend_path(); // Finder/Dock launches get a bare PATH: find gh/glab from Homebrew
    gpui_kit::application().with_assets(Assets).run(move |cx| {
        gpui_kit::init(cx);
        let platform = Platform::current();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(1040.), px(680.)), cx)),
            window_min_size: Some(size(px(760.), px(480.))),
            titlebar: Some(TitlebarOptions {
                title: Some("rebro".into()),
                // macOS: content runs under a transparent title bar, traffic lights over the sidebar
                appears_transparent: platform == Platform::Mac && cfg!(target_os = "macos"),
                traffic_light_position: (platform == Platform::Mac && cfg!(target_os = "macos")).then(|| point(px(18.), px(18.))),
            }),
            ..Default::default()
        };
        if let Err(e) = gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| Rebro::new(window, cx))) {
            eprintln!("rebro: can't open a window: {e:#}");
            cx.quit();
        }
    });
}
