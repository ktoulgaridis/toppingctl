//! The header's workspace combo (#17), 1:1 with the official app's:
//! Default, the named workspaces (each with Save and Delete), "New
//! workspace…", "Save workspace as…" and the auto-save toggle; `s` saves,
//! `l` (or a click on the combo) opens it. Keys and mouse, on the pure
//! `App`, carried out by `run::carry_out` against temp files and a
//! `Connection` over `FakeTransport`.
//!
//! Golden snapshots live in `tests/snapshots/`; run with
//! `UPDATE_SNAPSHOTS=1` to rewrite them after an intended change.

use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::Terminal;

use toppingctl::conn::fake::{FakeHandle, FakeTransport};
use toppingctl::conn::Connection;
use toppingctl::frame::{Dir, Frame};
use toppingctl::state::{Meters, State};
use toppingctl::sync::Store;
use toppingctl::tui::app::{Action, App, ComboItem, Modal, NameThen, FOLDER_ROW};
use toppingctl::tui::run::carry_out;
use toppingctl::tui::theme::Theme;
use toppingctl::tui::ui::{self, Hit, HitMap};
use toppingctl::workspace::{Settings, Workspaces};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ch(c: char) -> KeyEvent {
    key(KeyCode::Char(c))
}

fn render(app: &App, w: u16, h: u16) -> (Buffer, HitMap) {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut hits = HitMap::default();
    let meters = Meters::default();
    terminal.draw(|f| hits = ui::draw(f, app, &meters)).unwrap();
    (terminal.backend().buffer().clone(), hits)
}

fn text(buf: &Buffer) -> String {
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        let mut line = String::new();
        for x in 0..area.width {
            line.push_str(buf[(x, y)].symbol());
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn assert_snapshot(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        fs::write(&path, actual).unwrap();
        return;
    }
    let expected = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (run with UPDATE_SNAPSHOTS=1)", path.display()));
    assert!(
        expected == actual,
        "snapshot {name} differs\n--- expected\n{expected}--- actual\n{actual}"
    );
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("toppingctl-tuiws-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// The device: 48V known, a few settings.
fn device_state() -> State {
    let mut s = State::default();
    s.inputs[0].phantom = Some(false);
    s.inputs[1].phantom = Some(false);
    s.inputs[0].gain_db = 9;
    s.mixes[0].in1.fader_db = -6.0;
    s
}

struct Rig {
    dir: PathBuf,
    store: Store,
    ws: Workspaces,
    app: App,
    conn: Option<Connection>,
    fake: FakeHandle,
}

impl Rig {
    /// A synced app with the workspaces `names` saved (each from `State`
    /// with its own gain) and `current` open.
    fn new(name: &str, names: &[&str], current: Option<&str>) -> Self {
        let dir = temp_dir(name);
        let ws = Workspaces::at(dir.clone());
        for (i, n) in names.iter().enumerate() {
            let mut s = State::default();
            s.inputs[0].gain_db = i as u8 + 1;
            ws.save(n, &s).unwrap();
        }
        let settings = Settings {
            current: current.map(String::from),
            auto_save: false,
            ..Default::default()
        };
        ws.save_settings(&settings).unwrap();
        let (transport, fake) = FakeTransport::new();
        let (conn, _events) = Connection::spawn(transport);
        let mut app = App::new(Theme::color());
        app.synced(device_state());
        app.set_workspaces(ws.list(), ws.settings());
        app.set_workspaces_folder(ws.folder_shown());
        Rig {
            store: Store::at(dir.join("state.json")),
            ws,
            dir,
            app,
            conn: Some(conn),
            fake,
        }
    }

    fn run(&mut self, actions: Vec<Action>) -> Vec<Action> {
        carry_out(
            self.conn.as_ref().unwrap(),
            &mut self.app,
            &self.store,
            &self.ws,
            actions,
        )
        .unwrap()
    }

    fn key(&mut self, k: KeyEvent) -> Vec<Action> {
        let actions = self.app.handle_key(k);
        self.run(actions.clone());
        actions
    }

    fn type_text(&mut self, s: &str) {
        for c in s.chars() {
            self.key(ch(c));
        }
    }

    /// Renders at 120×40 and clicks `hit`.
    fn click(&mut self, hit: Hit) -> Vec<Action> {
        let (_, hits) = render(&self.app, 120, 40);
        let r = hits
            .rect(hit)
            .unwrap_or_else(|| panic!("{hit:?} not on screen"));
        let actions = click_at(&mut self.app, &hits, r);
        self.run(actions.clone());
        actions
    }

    fn written(&mut self) -> Vec<Frame> {
        drop(self.conn.take());
        self.fake
            .written()
            .iter()
            .map(|r| Frame::decode(r, Dir::ToDevice).unwrap())
            .collect()
    }

    fn saved(&self, name: &str) -> State {
        self.ws.load(name).unwrap()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn click_at(app: &mut App, hits: &HitMap, r: Rect) -> Vec<Action> {
    let at = |kind| MouseEvent {
        kind,
        column: r.x,
        row: r.y,
        modifiers: KeyModifiers::NONE,
    };
    let now = Instant::now();
    let mut actions = app.handle_mouse_at(at(MouseEventKind::Down(MouseButton::Left)), hits, now);
    actions.extend(app.handle_mouse_at(at(MouseEventKind::Up(MouseButton::Left)), hits, now));
    actions
}

/// The combo's index of `item`.
fn index_of(app: &App, item: ComboItem) -> usize {
    app.combo_items()
        .iter()
        .position(|&i| i == item)
        .unwrap_or_else(|| panic!("{item:?} not in the combo"))
}

/// Opens the combo with `l` and walks to `item`.
fn select(rig: &mut Rig, item: ComboItem) {
    rig.key(ch('l'));
    let i = index_of(&rig.app, item);
    assert_eq!(rig.app.modal(), Some(Modal::Workspaces { sel: 0 }));
    for _ in 0..i {
        rig.key(key(KeyCode::Down));
    }
    assert_eq!(rig.app.modal(), Some(Modal::Workspaces { sel: i }));
}

/// The header names the workspace and its keys work: no more labels.
#[test]
fn header() {
    let rig = Rig::new("header", &["gig"], Some("gig"));
    let (buf, hits) = render(&rig.app, 120, 40);
    let header = text(&buf).lines().take(4).collect::<Vec<_>>().join("\n");
    assert!(header.contains("gig ▾"), "{header}");
    assert!(header.contains("l workspaces"), "{header}");
    assert!(!header.contains("l load"), "{header}");
    for h in [Hit::Workspaces, Hit::SaveKey, Hit::Download] {
        assert!(hits.rect(h).is_some(), "{h:?}");
    }
    let rig = Rig::new("header-default", &[], None);
    let (buf, _) = render(&rig.app, 160, 50);
    let header = text(&buf).lines().take(4).collect::<Vec<_>>().join("\n");
    assert!(header.contains("workspace  Default ▾"), "{header}");
    assert!(header.contains("l workspaces"), "{header}");
}

/// The combo's items, in the app's order, and its snapshot.
#[test]
fn combo_items_and_snapshot() {
    let mut rig = Rig::new("items", &["gig", "studio"], Some("studio"));
    rig.key(ch('l'));
    assert_eq!(
        rig.app.combo_items(),
        vec![
            ComboItem::Default,
            ComboItem::Named(0),
            ComboItem::Named(1),
            ComboItem::New,
            ComboItem::SaveAs,
            ComboItem::AutoSave,
        ]
    );
    let (buf, _) = render(&rig.app, 120, 40);
    assert_snapshot("workspaces_combo_120x40", &text(&buf));
    rig.key(key(KeyCode::Esc));
    assert_eq!(rig.app.modal(), None);
    assert!(rig.written().is_empty());
}

/// Picking a workspace loads it (the diff, nothing else); picking Default
/// leaves the device alone and only closes the named one.
#[test]
fn pick_loads() {
    let mut rig = Rig::new("pick", &["gig", "studio"], None);
    select(&mut rig, ComboItem::Named(1));
    let actions = rig.key(key(KeyCode::Enter));
    assert_eq!(actions[0], Action::LoadWorkspace("studio".into()));
    assert_eq!(rig.app.workspace(), Some("studio"));
    assert_eq!(rig.app.state().inputs[0].gain_db, 2);
    assert_eq!(rig.app.modal(), None);

    select(&mut rig, ComboItem::Default);
    rig.key(key(KeyCode::Enter));
    assert_eq!(rig.app.workspace(), None);
    assert_eq!(rig.ws.settings().current, None);
    assert_eq!(
        rig.app.state().inputs[0].gain_db,
        2,
        "Default changes nothing"
    );

    let mut loaded = rig.saved("studio");
    loaded.inputs[0].phantom = Some(false);
    loaded.inputs[1].phantom = Some(false);
    assert_eq!(rig.written(), device_state().diff(&loaded));
}

/// `s` saves to the open workspace; on Default it asks for a name first.
#[test]
fn s_saves() {
    let mut rig = Rig::new("s", &["gig"], Some("gig"));
    let actions = rig.key(ch('s'));
    assert_eq!(actions, vec![Action::SaveWorkspace("gig".into())]);
    assert_eq!(rig.saved("gig").inputs[0].gain_db, 9);

    let mut rig = Rig::new("s-default", &[], None);
    rig.key(ch('s'));
    assert_eq!(
        rig.app.modal(),
        Some(Modal::WorkspaceName {
            then: NameThen::SaveAs
        })
    );
    // Separators and other unsafe characters are not taken.
    rig.type_text("my/set");
    assert_eq!(rig.app.prompt(), "myset");
    rig.key(key(KeyCode::Enter));
    assert_eq!(rig.app.modal(), None);
    assert_eq!(rig.app.workspace(), Some("myset"));
    assert_eq!(rig.ws.list(), vec!["myset".to_string()]);
    assert_eq!(rig.ws.settings().current.as_deref(), Some("myset"));
    assert!(rig.written().is_empty(), "a save sends nothing");
}

/// Save as: a new name saves and opens it; an existing one asks
/// "Overwrite?" first, and No keeps the file.
#[test]
fn save_as_and_overwrite() {
    let mut rig = Rig::new("save-as", &["gig"], None);
    select(&mut rig, ComboItem::SaveAs);
    rig.key(key(KeyCode::Enter));
    rig.type_text("gig");
    rig.key(key(KeyCode::Enter));
    assert_eq!(
        rig.app.modal(),
        Some(Modal::Overwrite {
            then: NameThen::SaveAs
        })
    );
    let (buf, _) = render(&rig.app, 120, 40);
    assert!(text(&buf).contains("Overwrite workspace \"gig\"?"));
    rig.key(ch('n'));
    assert_eq!(rig.app.modal(), None);
    assert_eq!(rig.saved("gig").inputs[0].gain_db, 1, "kept");

    select(&mut rig, ComboItem::SaveAs);
    rig.key(key(KeyCode::Enter));
    rig.type_text("gig");
    rig.key(key(KeyCode::Enter));
    rig.key(ch('y'));
    assert_eq!(rig.saved("gig").inputs[0].gain_db, 9, "overwritten");
    assert_eq!(rig.app.workspace(), Some("gig"));

    // An empty name is not taken; Esc cancels.
    select(&mut rig, ComboItem::SaveAs);
    rig.key(key(KeyCode::Enter));
    rig.key(key(KeyCode::Enter));
    assert!(matches!(rig.app.modal(), Some(Modal::WorkspaceName { .. })));
    rig.key(key(KeyCode::Esc));
    assert_eq!(rig.app.modal(), None);
    assert_eq!(rig.ws.list(), vec!["gig".to_string()]);
}

/// Each named entry's Save asks "Overwrite?"; its Delete asks "Remove
/// workspace?". Deleting the open one goes back to Default.
#[test]
fn entry_save_and_delete_keys() {
    let mut rig = Rig::new("entry", &["gig", "studio"], Some("studio"));
    select(&mut rig, ComboItem::Named(0));
    rig.key(ch('s'));
    assert_eq!(
        rig.app.modal(),
        Some(Modal::Overwrite {
            then: NameThen::SaveAs
        })
    );
    rig.key(ch('y'));
    assert_eq!(rig.saved("gig").inputs[0].gain_db, 9);
    assert_eq!(rig.app.workspace(), Some("gig"));

    select(&mut rig, ComboItem::Named(1));
    rig.key(ch('d'));
    assert_eq!(rig.app.modal(), Some(Modal::RemoveWorkspace));
    let (buf, _) = render(&rig.app, 120, 40);
    assert!(text(&buf).contains("Remove workspace \"studio\"?"));
    rig.key(ch('n'));
    assert_eq!(rig.ws.list().len(), 2, "No keeps it");

    select(&mut rig, ComboItem::Named(0));
    rig.key(key(KeyCode::Delete));
    rig.key(ch('y'));
    assert_eq!(rig.ws.list(), vec!["studio".to_string()]);
    assert_eq!(rig.app.workspaces(), &["studio".to_string()]);
    assert_eq!(rig.app.workspace(), None, "the open one went");
    assert_eq!(rig.ws.settings().current, None);
    assert!(rig.written().is_empty());
}

/// New workspace: a name, then the defaults (48V and the settings left as
/// they are), sent as a diff and saved under the name.
#[test]
fn new_workspace() {
    let mut rig = Rig::new("new", &[], None);
    select(&mut rig, ComboItem::New);
    rig.key(key(KeyCode::Enter));
    rig.type_text("fresh");
    rig.key(key(KeyCode::Enter));
    assert_eq!(rig.app.workspace(), Some("fresh"));
    let mut expected = State::default();
    expected.inputs[0].phantom = Some(false);
    expected.inputs[1].phantom = Some(false);
    assert_eq!(rig.app.state(), &expected);
    assert_eq!(rig.saved("fresh"), State::default());
    assert_eq!(rig.written(), device_state().diff(&expected));
}

/// Auto-save (off by default) writes every change to the open workspace.
#[test]
fn auto_save() {
    let mut rig = Rig::new("auto", &["gig"], Some("gig"));
    assert!(!rig.app.auto_save());
    select(&mut rig, ComboItem::AutoSave);
    rig.key(key(KeyCode::Enter));
    assert!(rig.app.auto_save());
    assert!(rig.ws.settings().auto_save);
    rig.key(key(KeyCode::Esc));
    // IN1 MON on (the first control): the whole state goes to "gig".
    rig.key(ch(' '));
    assert!(rig.saved("gig").inputs[0].mon);
    assert_eq!(rig.saved("gig").inputs[0].gain_db, 9);

    select(&mut rig, ComboItem::AutoSave);
    rig.key(key(KeyCode::Enter));
    assert!(!rig.ws.settings().auto_save);
    rig.key(key(KeyCode::Esc));
    rig.key(ch(' '));
    assert!(!rig.app.state().inputs[0].mon);
    assert!(rig.saved("gig").inputs[0].mon, "off again: not saved");
}

/// The same flows by mouse: open the combo, pick, Save / Delete with their
/// confirms, Save as with the OK button.
#[test]
fn mouse() {
    let mut rig = Rig::new("mouse", &["gig", "studio"], None);
    rig.click(Hit::Workspaces);
    assert!(matches!(rig.app.modal(), Some(Modal::Workspaces { .. })));
    rig.click(Hit::ComboItem(index_of(&rig.app, ComboItem::Named(0))));
    assert_eq!(rig.app.workspace(), Some("gig"));
    assert_eq!(rig.app.modal(), None);

    rig.click(Hit::Workspaces);
    rig.click(Hit::ComboSave(index_of(&rig.app, ComboItem::Named(1))));
    assert!(matches!(rig.app.modal(), Some(Modal::Overwrite { .. })));
    rig.click(Hit::Yes);
    assert_eq!(rig.saved("studio").inputs[0].gain_db, 1);
    assert_eq!(rig.app.workspace(), Some("studio"));

    rig.click(Hit::Workspaces);
    rig.click(Hit::ComboDelete(index_of(&rig.app, ComboItem::Named(0))));
    assert_eq!(rig.app.modal(), Some(Modal::RemoveWorkspace));
    rig.click(Hit::No);
    assert_eq!(rig.ws.list().len(), 2);
    rig.click(Hit::Workspaces);
    rig.click(Hit::ComboDelete(index_of(&rig.app, ComboItem::Named(0))));
    rig.click(Hit::Yes);
    assert_eq!(rig.ws.list(), vec!["studio".to_string()]);

    rig.click(Hit::Workspaces);
    rig.click(Hit::ComboItem(index_of(&rig.app, ComboItem::SaveAs)));
    assert!(matches!(rig.app.modal(), Some(Modal::WorkspaceName { .. })));
    let (buf, _) = render(&rig.app, 120, 40);
    assert_snapshot("workspace_name_120x40", &text(&buf));
    rig.type_text("mouse set");
    rig.click(Hit::Yes);
    assert_eq!(rig.app.workspace(), Some("mouse set"));
    assert!(rig.ws.list().contains(&"mouse set".to_string()));

    rig.click(Hit::Workspaces);
    rig.click(Hit::ComboItem(index_of(&rig.app, ComboItem::AutoSave)));
    assert!(rig.app.auto_save());
    rig.click(Hit::No);
    assert_eq!(rig.app.modal(), None);

    // The header's `s` saves the open one.
    rig.app.set_notice(None);
    let actions = rig.click(Hit::SaveKey);
    assert_eq!(actions, vec![Action::SaveWorkspace("mouse set".into())]);
}

// ------------------------------------------- the workspaces folder (#57)

/// Opens Settings with `e` and walks down to the Workspaces folder row.
fn to_folder_row(rig: &mut Rig) {
    rig.key(ch('e'));
    for _ in 0..FOLDER_ROW {
        rig.key(key(KeyCode::Down));
    }
    assert_eq!(rig.app.modal(), Some(Modal::Settings { sel: FOLDER_ROW }));
}

/// Enter on the folder row edits the path, filled in with the folder;
/// typing (`q` and `e` too) and Backspace edit it, Esc leaves it as it
/// was and goes back to the row.
#[test]
fn folder_edit_keys() {
    let mut rig = Rig::new("folder-keys", &["gig"], Some("gig"));
    let shown = rig.ws.folder_shown();
    to_folder_row(&mut rig);
    let (buf, _) = render(&rig.app, 120, 40);
    let t = text(&buf);
    assert!(t.contains("Workspaces folder"), "{t}");
    assert!(t.contains("[Change]"), "{t}");
    assert!(rig.key(key(KeyCode::Right)).is_empty(), "nothing to step");
    assert!(rig.key(key(KeyCode::Enter)).is_empty());
    assert_eq!(rig.app.modal(), Some(Modal::WorkspaceFolder));
    assert_eq!(rig.app.prompt(), shown);
    rig.type_text("/qe");
    assert_eq!(rig.app.prompt(), format!("{shown}/qe"));
    rig.key(key(KeyCode::Backspace));
    assert_eq!(rig.app.prompt(), format!("{shown}/q"));
    assert_eq!(rig.app.modal(), Some(Modal::WorkspaceFolder), "q types");
    assert!(rig.key(key(KeyCode::Esc)).is_empty());
    assert_eq!(rig.app.modal(), Some(Modal::Settings { sel: FOLDER_ROW }));
    assert_eq!(rig.ws.settings().folder, None);
    assert_eq!(rig.app.workspace(), Some("gig"));
}

/// Enter keeps the typed folder: created if missing, kept in
/// `workspaces.json` with the open workspace and auto-save, and the list
/// reads it. Nothing moves; the open workspace, not in the new folder,
/// falls back to Default.
#[test]
fn folder_change_reads_the_new_folder() {
    let mut rig = Rig::new("folder-change", &["gig", "studio"], Some("gig"));
    rig.app.set_workspaces(rig.ws.list(), {
        let mut s = rig.ws.settings();
        s.auto_save = true;
        s
    });
    rig.run(vec![Action::SaveWorkspaceSettings]);
    // Another folder holding a copy of "studio".
    let new = rig.dir.join("new place");
    fs::create_dir_all(&new).unwrap();
    let old = rig.dir.join("workspaces");
    fs::copy(old.join("studio.json"), new.join("studio.json")).unwrap();
    let deeper = new.join("sub");

    to_folder_row(&mut rig);
    rig.key(key(KeyCode::Enter));
    while !rig.app.prompt().is_empty() {
        rig.key(key(KeyCode::Backspace));
    }
    rig.type_text(&deeper.display().to_string());
    let actions = rig.key(key(KeyCode::Enter));
    assert_eq!(
        actions,
        vec![Action::SetWorkspaceFolder(deeper.display().to_string())]
    );
    assert_eq!(rig.app.modal(), Some(Modal::Settings { sel: FOLDER_ROW }));
    assert!(deeper.is_dir(), "created");
    let saved = Workspaces::at(rig.dir.clone()).settings();
    assert_eq!(saved.folder, Some(deeper.clone()));
    assert!(saved.auto_save, "auto-save kept");
    assert_eq!(saved.current, None, "gig isn't there: Default");
    assert!(rig.app.workspaces().is_empty());
    assert_eq!(rig.app.workspace(), None);
    assert_eq!(rig.app.workspaces_folder(), deeper.display().to_string());
    // The old folder is as it was.
    assert!(old.join("gig.json").is_file() && old.join("studio.json").is_file());
    let (buf, _) = render(&rig.app, 120, 40);
    assert!(text(&buf).contains("sub"), "the row shows the new folder");

    // Empty is the default folder again: the setting goes.
    rig.run(vec![Action::SetWorkspaceFolder(String::new())]);
    assert_eq!(rig.ws.settings().folder, None);
    assert_eq!(rig.app.workspaces(), ["gig".to_string(), "studio".into()]);
    assert_eq!(rig.app.workspaces_folder(), rig.ws.folder_shown());

    // A folder holding the open workspace keeps it open.
    rig.app.set_workspaces(rig.ws.list(), {
        let mut s = rig.ws.settings();
        s.current = Some("studio".into());
        s
    });
    rig.run(vec![Action::SetWorkspaceFolder(new.display().to_string())]);
    assert_eq!(rig.app.workspaces(), ["studio".to_string()]);
    assert_eq!(rig.app.workspace(), Some("studio"));
    assert_eq!(rig.ws.settings().current.as_deref(), Some("studio"));
    assert_eq!(rig.ws.settings().folder, Some(new.clone()));
    assert!(rig.written().is_empty(), "nothing reaches the device");
}

/// A folder that can't be one (a relative path) changes nothing and says
/// why.
#[test]
fn folder_change_refused() {
    let mut rig = Rig::new("folder-bad", &["gig"], Some("gig"));
    rig.run(vec![Action::SetWorkspaceFolder("not/absolute".into())]);
    let notice = rig.app.notice().expect("a notice").to_string();
    assert!(notice.contains("not/absolute"), "{notice}");
    assert_eq!(rig.ws.settings().folder, None);
    assert_eq!(rig.app.workspace(), Some("gig"));
    assert_eq!(rig.app.workspaces(), ["gig".to_string()]);
}

/// A click on `[Change]` edits the folder; the dialog's close button,
/// while editing, goes back to the row.
#[test]
fn folder_change_click() {
    let mut rig = Rig::new("folder-click", &[], None);
    rig.key(ch('e'));
    rig.click(Hit::FolderChange);
    assert_eq!(rig.app.modal(), Some(Modal::WorkspaceFolder));
    assert_eq!(rig.app.prompt(), rig.ws.folder_shown());
    rig.click(Hit::No);
    assert_eq!(rig.app.modal(), Some(Modal::Settings { sel: FOLDER_ROW }));
}

/// "Import from TOPPING app…" (#58): hidden when the official app's folder
/// has no E2x2 workspaces (as on Linux); else it lists them, and Enter (or
/// a click) saves the chosen one as a toppingctl workspace, without
/// loading it or sending anything. A name that exists is not replaced.
#[test]
fn import_from_the_official_app() {
    use toppingctl::tpwork::Official;
    let mut rig = Rig::new("import", &["gig"], None);
    assert!(!rig.app.combo_items().contains(&ComboItem::Import));

    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/official/E2X2_New202608221427.TPwork");
    let other = rig.dir.join("E2X2_gig.TPwork");
    fs::copy(&fixture, &other).unwrap();
    let found = vec![
        Official {
            name: "New202608221427".into(),
            path: fixture.clone(),
        },
        Official {
            name: "gig".into(),
            path: other,
        },
    ];
    rig.app.set_official(found);
    let items = rig.app.combo_items();
    assert_eq!(
        &items[items.len() - 2..],
        [ComboItem::Import, ComboItem::AutoSave]
    );

    select(&mut rig, ComboItem::Import);
    rig.key(key(KeyCode::Enter));
    assert_eq!(rig.app.modal(), Some(Modal::Import { sel: 0 }));
    let (buf, _) = render(&rig.app, 120, 40);
    assert_snapshot("workspaces_import_120x40", &text(&buf));
    let actions = rig.key(key(KeyCode::Enter));
    assert_eq!(actions, vec![Action::ImportWorkspace(fixture)]);
    assert_eq!(rig.app.modal(), None);
    assert_eq!(rig.app.workspaces(), ["New202608221427", "gig"]);
    assert_eq!(rig.app.workspace(), None, "imported, not opened");
    let notice = rig.app.notice().unwrap_or_default().to_string();
    assert!(notice.contains("imported \"New202608221427\""), "{notice}");
    let s = rig.saved("New202608221427");
    assert_eq!((s.inputs[0].gain_db, s.inputs[0].mute), (2, true));

    // "gig" exists: not replaced, and the notice says so. By mouse.
    select(&mut rig, ComboItem::Import);
    rig.key(key(KeyCode::Enter));
    rig.click(Hit::ImportOption(1));
    let notice = rig.app.notice().unwrap_or_default().to_string();
    assert!(notice.contains("already exists"), "{notice}");
    assert_eq!(rig.saved("gig").inputs[0].gain_db, 1, "kept");

    // Esc closes the list.
    select(&mut rig, ComboItem::Import);
    rig.key(key(KeyCode::Enter));
    rig.key(key(KeyCode::Esc));
    assert_eq!(rig.app.modal(), None);
    assert!(rig.written().is_empty(), "an import sends nothing");
}
