//! The generated types describe the JSON serde writes and reads.
//!
//! - `json_shapes` pins that JSON for each kind of type (enums with and
//!   without data, unit variants inside data enums, `Option`, `Vec`, the
//!   recursive `Layout`), and that serde reads what the apps send.
//! - `typescript_accepts_the_json` writes every sample as a TypeScript literal
//!   of its type and type-checks it against `api.ts` with `tsc --strict`, when
//!   there is one: `tsc` on the PATH, or a command in `SHED_TSC`
//!   (`SHED_TSC="npx -y -p typescript@5 tsc"`). Otherwise it says it skipped.
//! - `generated_files_are_current` fails when shed-api changed without
//!   `cargo run -p shed-codegen`.

use std::process::Command;

use serde::Serialize;
use serde_json::{Value, json};
use shed_api::calls::*;
use shed_api::http::{PairApproved, PairState, PushPrefs, SignedIn};
use shed_api::state::{
    AiActivity, AiDraft, AiTab, Bench, ClosedBench, Divider, Layout, LimitWindow, Limits, Link,
    Pane, ProjectTab, RemoteTab, Split, SplitDir, Tab, TabKind, Tag, WorkspaceState,
};
use shed_api::topics::{
    AiAssistant, AiEntriesChanged, AiEntry, AiEntryAt, AiNote, AiNoteKind, AiNotice, AiOption,
    AiPermission, AiQuestion, AiRun, AiRunState, AiStatus, AiTextEdit, AiTimer, AiTimerKind,
    AiTool, AiTranscript, AiUser, Cursor, Event, Highlight, ScreenLine, ScreenRow, SearchTopic,
    Span, Topic,
};
use shed_api::wire::*;
use shed_api::{ApiError, ErrorCode};

fn to_json<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap()
}

fn layout() -> Layout {
    Layout::Split(Split {
        dir: SplitDir::Right,
        ratio: 0.5,
        first: Box::new(Layout::Pane("p1".into())),
        second: Box::new(Layout::Split(Split {
            dir: SplitDir::Down,
            ratio: 0.25,
            first: Box::new(Layout::Pane("p2".into())),
            second: Box::new(Layout::Pane("p3".into())),
        })),
    })
}

fn state() -> WorkspaceState {
    let pane = |id: &str| Pane {
        id: id.into(),
        cwd: "/home/you/shed".into(),
        cols: 80,
        rows: 24,
        tool: Some("vim notes.md".into()),
        ..Default::default()
    };
    let terminal = Tab {
        id: "t1".into(),
        name: Some("build".into()),
        title: "build".into(),
        tags: vec![Tag { label: "prod".into(), color: "#ff6b5b".into(), tint: true }],
        kind: TabKind::Terminal,
        panes: vec![pane("p1"), pane("p2"), pane("p3")],
        layout: Some(layout()),
        active_pane: Some("p1".into()),
        divider: Divider::Space,
        ..Default::default()
    };
    let project = Tab {
        id: "t2".into(),
        title: "shed".into(),
        kind: TabKind::Project(ProjectTab {
            root: "/home/you/shed".into(),
            open: vec!["src/main.rs".into()],
            active: Some(0),
            links: vec![],
        }),
        ..Default::default()
    };
    let ai = Tab {
        id: "t3".into(),
        title: "Claude".into(),
        private: true,
        kind: TabKind::Ai(AiTab {
            folder: "/home/you/shed".into(),
            mode: "default".into(),
            activity: AiActivity::Waiting,
            drafts: vec![AiDraft { session: "s1".into(), text: "later".into() }],
            ..Default::default()
        }),
        ..Default::default()
    };
    let remote = Tab {
        id: "t4".into(),
        title: "server".into(),
        kind: TabKind::Remote(RemoteTab { host: "server".into(), id: "r1".into() }),
        ..Default::default()
    };
    WorkspaceState {
        host: "desk".into(),
        benches: vec![Bench {
            id: "b1".into(),
            name: "shed".into(),
            color: "#4a90d9".into(),
            cwd: "/home/you/shed".into(),
            tabs: vec![terminal, project, ai, remote],
            active_tab: "t1".into(),
            shown: true,
            links: vec![Link { name: "CI".into(), url: "https://example.com".into() }],
            ..Default::default()
        }],
        closed: vec![ClosedBench {
            name: "notes".into(),
            color: "#9b59b6".into(),
            badge: Some("N".into()),
            cwd: "/home/you/notes".into(),
            tabs: vec![],
            saved: 1_763_500_000,
            links: vec![],
        }],
        limits: Some(Limits {
            five_hour: Some(LimitWindow { used: 0.5, resets: 1_763_510_000 }),
            week: None,
            seen: 1_763_500_000,
        }),
        home: "/home/you".into(),
    }
}

fn request(id: u32, call: Call) -> ClientFrame {
    ClientFrame::Request(Request { id, workspace: "w1".into(), call })
}

fn reply(id: u32, reply: Reply) -> ServerFrame {
    ServerFrame::Response(Response::ok(id, reply))
}

/// Samples of every kind of shape, as (TypeScript type, JSON).
fn samples() -> Vec<(&'static str, Value)> {
    let workspace = WorkspaceInfo {
        id: "w1".into(),
        name: "desk".into(),
        online: true,
        seen: 1_763_500_000,
        policy: Policy::View,
        version: "0.1.0".into(),
    };
    let transcript = AiTranscript {
        from: 0,
        entries: vec![
            AiEntry::User(AiUser { text: "hi".into(), at: 1, pictures: vec![], images: 0 }),
            AiEntry::Assistant(AiAssistant { text: "**hello**".into(), at: 2 }),
            AiEntry::Tool(AiTool {
                id: "tool1".into(),
                name: "Edit".into(),
                summary: "Edit src/main.rs".into(),
                detail: String::new(),
                file: Some("src/main.rs".into()),
                edit: Some(AiTextEdit { old: "a".into(), new: "b".into() }),
                result: None,
                error: false,
                pictures: vec![],
                run: None,
            }),
            AiEntry::Tool(AiTool {
                id: "tool2".into(),
                name: "Agent".into(),
                summary: "Agent: Count files".into(),
                detail: "Count the files".into(),
                file: None,
                edit: None,
                result: None,
                error: false,
                pictures: vec![],
                run: Some(AiRun {
                    state: AiRunState::Running,
                    background: true,
                    agent: "Explore".into(),
                    started: 1_763_500_000,
                    last: "$ ls".into(),
                    tools: 3,
                    tokens: 1200.0,
                    steps: vec![AiEntry::Assistant(AiAssistant { text: "Looking".into(), at: 0 })],
                    skipped: 2,
                    ..Default::default()
                }),
            }),
            AiEntry::Note(AiNote {
                kind: AiNoteKind::Wakeup,
                title: "Woke up: check the build".into(),
                detail: String::new(),
                error: false,
                at: 3,
            }),
            AiEntry::Permission(AiPermission {
                request: "q1".into(),
                tool: "AskUserQuestion".into(),
                summary: "Which?".into(),
                detail: String::new(),
                edit: None,
                answer: Some(true),
                questions: vec![AiQuestion {
                    question: "Which one?".into(),
                    header: "Pick".into(),
                    options: vec![AiOption { label: "A".into(), description: String::new() }],
                    multi: false,
                }],
                answers: vec!["A".into()],
            }),
            AiEntry::Notice(AiNotice { text: "Interrupted".into(), error: true }),
        ],
        status: AiStatus {
            session: Some("s1".into()),
            state: AiActivity::Working,
            cost: 0.12,
            since: Some(1_763_500_000),
            timers: vec![AiTimer {
                kind: AiTimerKind::Job,
                what: "Say tick".into(),
                at: 1_763_500_060,
                recurring: true,
                every: "Every minute".into(),
                prompt: "Say tick".into(),
                tool: "tool9".into(),
            }],
            ..Default::default()
        },
    };
    let changed = AiEntriesChanged {
        entries: vec![AiEntryAt { index: 3, entry: transcript.entries[3].clone() }],
        status: transcript.status.clone(),
    };
    let screen = shed_api::topics::Screen {
        full: true,
        cols: 80,
        rows: 24,
        lines: vec![ScreenRow {
            row: 0,
            line: ScreenLine {
                spans: vec![Span {
                    text: "$ ls".into(),
                    fg: Some("#ff0000".into()),
                    bold: true,
                    ..Default::default()
                }],
                wraps: false,
            },
        }],
        cursor: Cursor { row: 0, col: 4, visible: true },
        ..Default::default()
    };
    let file = FileContent {
        path: "src/main.rs".into(),
        kind: FileKind::TooLarge,
        text: "fn main() {}".into(),
        lang: Some("rust".into()),
        highlights: vec![vec![Highlight { start: 0, end: 2, kind: "keyword".into() }], vec![]],
        picture: None,
        size: 3_000_000_000.0,
    };
    let diff = Diff {
        hunks: vec![Hunk {
            header: "@@ -1 +1 @@".into(),
            lines: vec![
                DiffLine { kind: DiffLineKind::Removed, text: "a".into(), old: Some(1), new: None },
                DiffLine { kind: DiffLineKind::Added, text: "b".into(), old: None, new: Some(1) },
            ],
        }],
        new_picture: Some(Picture { media_type: "image/png".into(), data: "iVBO".into() }),
        ..Default::default()
    };

    vec![
        (
            "ServerFrame",
            to_json(&ServerFrame::Welcome(Welcome {
                api: 1,
                nexus: "0.1.0".into(),
                workspaces: vec![workspace.clone()],
                states: vec![StateFrame { workspace: "w1".into(), state: state() }],
            })),
        ),
        ("ServerFrame", to_json(&reply(1, Reply::Done))),
        ("ServerFrame", to_json(&reply(2, Reply::Bench("b1".into())))),
        ("ServerFrame", to_json(&reply(3, Reply::File(file)))),
        ("ServerFrame", to_json(&reply(4, Reply::Diff(diff)))),
        (
            "ServerFrame",
            to_json(&reply(
                5,
                Reply::Log(vec![LogEntry {
                    command: "false".into(),
                    cwd: "/".into(),
                    exit: Some(-1),
                    started: 1,
                    duration: 1.5,
                    bench: String::new(),
                    tab: String::new(),
                }]),
            )),
        ),
        (
            "ServerFrame",
            to_json(&ServerFrame::Response(Response::error(
                6,
                ApiError::new(ErrorCode::NotFound, "no such tab"),
            ))),
        ),
        (
            "ServerFrame",
            to_json(&ServerFrame::Event(EventFrame { sub: 1, event: Event::GitChanged })),
        ),
        (
            "ServerFrame",
            to_json(&ServerFrame::Event(EventFrame { sub: 2, event: Event::Screen(screen) })),
        ),
        (
            "ServerFrame",
            to_json(&ServerFrame::Event(EventFrame {
                sub: 3,
                event: Event::AiTranscript(transcript),
            })),
        ),
        (
            "ServerFrame",
            to_json(&ServerFrame::Event(EventFrame { sub: 3, event: Event::AiEntries(changed) })),
        ),
        ("ServerFrame", to_json(&ServerFrame::Workspaces(vec![workspace]))),
        ("ServerFrame", to_json(&ServerFrame::Ended(Ended { sub: 1, error: None }))),
        ("ServerFrame", to_json(&ServerFrame::Goodbye(ApiError::new(ErrorCode::Unauthorized, "")))),
        (
            "ServerFrame",
            to_json(&ServerFrame::Notification(NotificationFrame {
                workspace: "w1".into(),
                notification: Notification {
                    kind: NotificationKind::AiWaiting,
                    title: "Claude waits for an answer".into(),
                    body: "api · claude".into(),
                    bench: Some("b1".into()),
                    tab: None,
                    present: false,
                },
            })),
        ),
        ("ClientFrame", to_json(&ClientFrame::Visible(true))),
        (
            "ClientFrame",
            to_json(&ClientFrame::Hello(ClientHello { api: 1, client: "web 0.1.0".into() })),
        ),
        ("ClientFrame", to_json(&request(1, Call::Previews))),
        (
            "ClientFrame",
            to_json(&request(
                9,
                Call::AiUpload(AiUpload { tab: "t3".into(), name: "spec.pdf".into(), data: "JVBE".into() }),
            )),
        ),
        (
            "ClientFrame",
            to_json(&request(
                2,
                Call::Input(Input {
                    pane: "p1".into(),
                    input: vec![
                        InputItem::Text("ls".into()),
                        InputItem::Key(Key {
                            key: "Enter".into(),
                            ctrl: false,
                            alt: false,
                            shift: true,
                        }),
                        InputItem::Paste("echo hi".into()),
                    ],
                }),
            )),
        ),
        (
            "ClientFrame",
            to_json(&request(
                3,
                Call::GitDiff(GitDiff {
                    tab: "t2".into(),
                    path: "a.rs".into(),
                    source: DiffSource::Staged,
                }),
            )),
        ),
        (
            "ClientFrame",
            to_json(&request(
                4,
                Call::GitHunk(GitHunk {
                    tab: "t2".into(),
                    path: "a.rs".into(),
                    source: DiffSource::Commit("abc123".into()),
                    hunk: 0,
                    header: "@@ -1 +1 @@".into(),
                    action: HunkAction::Discard,
                }),
            )),
        ),
        (
            "ClientFrame",
            to_json(&request(
                5,
                Call::NewTab(NewTab {
                    bench: BenchRef::Running("b1".into()),
                    kind: NewTabKind::Terminal(OptionalFolder { folder: None }),
                }),
            )),
        ),
        (
            "ClientFrame",
            to_json(&request(
                6,
                Call::AiSettings(AiSettings {
                    tab: "t3".into(),
                    model: Some(String::new()),
                    mode: None,
                    effort: None,
                    follow: Some(true),
                    browser: None,
                    add_dirs: Some(vec!["/tmp".into()]),
                }),
            )),
        ),
        (
            "ClientFrame",
            to_json(&request(
                7,
                Call::AiEdit(AiEdit {
                    tab: "t3".into(),
                    entry: 2,
                    text: "again".into(),
                    pictures: vec![],
                }),
            )),
        ),
        (
            "ClientFrame",
            to_json(&ClientFrame::Subscribe(Subscribe {
                id: 8,
                workspace: "w1".into(),
                topic: Topic::Search(SearchTopic {
                    tab: "t2".into(),
                    query: "fn main".into(),
                    regex: false,
                    case_sensitive: true,
                    whole_word: false,
                    include: Some("*.rs".into()),
                    exclude: None,
                }),
            })),
        ),
        ("LinkFrame", to_json(&LinkFrame::State(state()))),
        (
            "LinkFrame",
            to_json(&LinkFrame::Notify(Notification {
                kind: NotificationKind::AiWaiting,
                title: "Claude waits for an answer".into(),
                body: String::new(),
                bench: Some("b1".into()),
                tab: None,
                present: true,
            })),
        ),
        (
            "NexusFrame",
            to_json(&NexusFrame::Welcome(LinkWelcome {
                api: 1,
                nexus: "0.1.0".into(),
                workspace: "w1".into(),
                name: "desk".into(),
            })),
        ),
        ("PairState", to_json(&PairState::Pending)),
        (
            "PairState",
            to_json(&PairState::Approved(PairApproved {
                workspace: "w1".into(),
                name: "desk".into(),
                token: "secret".into(),
            })),
        ),
        (
            "SignedIn",
            to_json(&SignedIn {
                username: "you".into(),
                device: "d1".into(),
                token: None,
                csrf: Some("c".into()),
                recovery_codes: vec!["r1".into()],
                expires: None,
            }),
        ),
        (
            "PushPrefs",
            to_json(&PushPrefs {
                kinds: vec![NotificationKind::AiDone, NotificationKind::WorkspaceOffline],
            }),
        ),
    ]
}

#[test]
fn json_shapes() {
    // Enums with data: {"type", "content"}; a unit variant has no content.
    assert_eq!(to_json(&Reply::Done), json!({"type": "Done"}));
    assert_eq!(to_json(&Reply::Bench("b1".into())), json!({"type": "Bench", "content": "b1"}));
    assert_eq!(to_json(&PairState::Pending), json!({"type": "Pending"}));
    assert_eq!(to_json(&Event::GitChanged), json!({"type": "GitChanged"}));
    assert_eq!(to_json(&TabKind::Terminal), json!({"type": "Terminal"}));
    assert_eq!(
        to_json(&DiffSource::Commit("abc".into())),
        json!({"type": "Commit", "content": "abc"})
    );
    assert_eq!(to_json(&Reply::Previews(vec![])), json!({"type": "Previews", "content": []}));
    assert_eq!(to_json(&ClientFrame::Visible(false)), json!({"type": "Visible", "content": false}));
    // A struct as content, with camelCase fields.
    assert_eq!(
        to_json(&TabKind::Remote(RemoteTab { host: "h".into(), id: "r".into() })),
        json!({"type": "Remote", "content": {"host": "h", "id": "r"}})
    );
    assert_eq!(
        to_json(&request(1, Call::Previews)),
        json!({"type": "Request", "content": {"id": 1, "workspace": "w1", "call": {"type": "Previews"}}})
    );

    // Enums without data: camelCase strings.
    assert_eq!(to_json(&Policy::View), json!("view"));
    assert_eq!(to_json(&NotificationKind::AiWaiting), json!("aiWaiting"));
    assert_eq!(to_json(&FileKind::TooLarge), json!("tooLarge"));
    assert_eq!(to_json(&Divider::None), json!("none"));

    // `None` is written as null; Vec as an array.
    assert_eq!(
        to_json(&Response::ok(1, Reply::Done)),
        json!({"id": 1, "ok": {"type": "Done"}, "error": null})
    );
    assert_eq!(
        to_json(&Pane { id: "p".into(), size_held: true, ..Default::default() }),
        json!({
            "id": "p", "cwd": "", "cols": 0, "rows": 0, "title": null, "tool": null,
            "waiting": null, "held": false, "asking": null, "sizeHeld": true
        })
    );

    // Box: the layout nests.
    assert_eq!(
        to_json(&layout()),
        json!({"type": "Split", "content": {
            "dir": "right", "ratio": 0.5,
            "first": {"type": "Pane", "content": "p1"},
            "second": {"type": "Split", "content": {
                "dir": "down", "ratio": 0.25,
                "first": {"type": "Pane", "content": "p2"},
                "second": {"type": "Pane", "content": "p3"},
            }},
        }})
    );

    // What the apps send: TypeScript leaves out undefined fields, Kotlin
    // (explicitNulls = false) leaves out nulls and defaults.
    let sent: ClientFrame = serde_json::from_value(json!({"type": "Request", "content": {
        "id": 7,
        "call": {"type": "NewTab", "content": {
            "bench": {"type": "Running", "content": "b1"},
            "kind": {"type": "Terminal", "content": {}},
        }},
    }}))
    .unwrap();
    assert_eq!(
        sent,
        ClientFrame::Request(Request {
            id: 7,
            workspace: String::new(),
            call: Call::NewTab(NewTab {
                bench: BenchRef::Running("b1".into()),
                kind: NewTabKind::Terminal(OptionalFolder { folder: None }),
            }),
        })
    );
    let hello: ClientFrame =
        serde_json::from_value(json!({"type": "Hello", "content": {"api": 1}})).unwrap();
    assert_eq!(hello, ClientFrame::Hello(ClientHello { api: 1, client: String::new() }));
    let done: Reply = serde_json::from_value(json!({"type": "Done"})).unwrap();
    assert_eq!(done, Reply::Done);
    let response: Response = serde_json::from_value(json!({"id": 1, "ok": null})).unwrap();
    assert_eq!(response, Response { id: 1, ok: None, error: None });
}

/// Every sample reads back as what wrote it (the JSON is the whole story).
#[test]
fn samples_read_back() {
    for (ty, value) in samples() {
        let text = value.to_string();
        let again = match ty {
            "ServerFrame" => to_json(&serde_json::from_str::<ServerFrame>(&text).unwrap()),
            "ClientFrame" => to_json(&serde_json::from_str::<ClientFrame>(&text).unwrap()),
            "LinkFrame" => to_json(&serde_json::from_str::<LinkFrame>(&text).unwrap()),
            "NexusFrame" => to_json(&serde_json::from_str::<NexusFrame>(&text).unwrap()),
            "PairState" => to_json(&serde_json::from_str::<PairState>(&text).unwrap()),
            "SignedIn" => to_json(&serde_json::from_str::<SignedIn>(&text).unwrap()),
            "PushPrefs" => to_json(&serde_json::from_str::<PushPrefs>(&text).unwrap()),
            other => panic!("no reader for {other}"),
        };
        assert_eq!(again, value, "{ty}");
    }
}

/// The TypeScript checker, if there is one: `SHED_TSC`, else `tsc`.
fn tsc() -> Option<Vec<String>> {
    if let Ok(command) = std::env::var("SHED_TSC") {
        return Some(command.split_whitespace().map(String::from).collect());
    }
    let found = Command::new("tsc").arg("--version").output().is_ok_and(|o| o.status.success());
    found.then(|| vec!["tsc".into()])
}

#[test]
fn typescript_accepts_the_json() {
    let root = shed_codegen::workspace_root().canonicalize().unwrap();
    let api = root.join("web/src/api/gen/api");
    let mut ts = format!("import type * as api from {:?};\n\n", api.display().to_string());
    for (i, (ty, value)) in samples().iter().enumerate() {
        let literal = serde_json::to_string_pretty(value).unwrap();
        ts += &format!("export const sample{i}: api.{ty} = {literal};\n\n");
    }
    // The types are strict, not `any`: these must fail.
    ts += "\
// @ts-expect-error: a variant with data needs its content
export const bad0: api.Reply = { type: \"Bench\" };
// @ts-expect-error: a unit variant has no content
export const bad1: api.Reply = { type: \"Done\", content: \"b1\" };
// @ts-expect-error: enums without data are camelCase strings
export const bad2: api.Policy = \"View\";
// @ts-expect-error: no such field
export const bad3: api.ClientHello = { api: 1, client: \"\", extra: 1 };
// @ts-expect-error: no such variant
export const bad4: api.Event = { type: \"Nope\" };
// @ts-expect-error: the content's type is checked
export const bad5: api.Layout = { type: \"Pane\", content: 1 };
";
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("codegen-check");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("check.ts");
    std::fs::write(&file, ts).unwrap();

    let Some(command) = tsc() else {
        eprintln!(
            "skipped: no tsc (set SHED_TSC=\"npx -y -p typescript@5 tsc\" to check {})",
            file.display()
        );
        return;
    };
    let output = Command::new(&command[0])
        .args(&command[1..])
        .args(["--noEmit", "--strict", "--target", "es2022", "--module", "es2022"])
        .args(["--moduleResolution", "bundler"])
        .arg(&file)
        .output()
        .expect("running tsc");
    assert!(
        output.status.success(),
        "tsc rejected {}:\n{}{}",
        file.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn kotlin_sealed_classes() {
    let kt = shed_codegen::kotlin(&shed_codegen::workspace_root()).unwrap();
    // A unit variant is an object: {"type": "Done"}.
    assert!(kt.contains("@SerialName(\"Done\")\n\tobject Done: Reply()"));
    assert!(kt.contains("@SerialName(\"GitChanged\")\n\tobject GitChanged: Event()"));
    // A variant with data: {"type": "Bench", "content": "…"}.
    assert!(
        kt.contains("@SerialName(\"Bench\")\n\tdata class Bench(val content: String): Reply()")
    );
    // Content types a variant's name would shadow carry their package.
    assert!(kt.contains("data class Screen(val content: dev.shed.api.Screen): Event()"));
    assert!(kt.contains("data class Split(val content: dev.shed.api.Split): Layout()"));
    assert!(kt.contains("data class Previews(val content: List<Preview>): Reply()"));
    // Enums without data read camelCase strings.
    assert!(kt.contains("@SerialName(\"aiWaiting\")\n\tAiWaiting(\"aiWaiting\")"));
}

#[test]
fn generated_files_are_current() {
    let root = shed_codegen::workspace_root();
    let read = |file: &str| std::fs::read_to_string(root.join(file)).unwrap_or_default();
    let stale = "is not what shed-api makes now: run `cargo run -p shed-codegen`";
    assert!(
        read(shed_codegen::TYPESCRIPT_FILE) == shed_codegen::typescript(&root).unwrap(),
        "{} {stale}",
        shed_codegen::TYPESCRIPT_FILE
    );
    assert!(
        read(shed_codegen::KOTLIN_FILE) == shed_codegen::kotlin(&root).unwrap(),
        "{} {stale}",
        shed_codegen::KOTLIN_FILE
    );
}
