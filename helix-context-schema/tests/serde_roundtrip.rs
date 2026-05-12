use helix_context_schema::{Active, Cursor, OpenBuffer, Position, Selection, UpdateSource};

#[test]
fn update_source_serializes_to_snake_case() {
    assert_eq!(
        serde_json::to_string(&UpdateSource::FocusLost).unwrap(),
        "\"focus_lost\""
    );
    assert_eq!(
        serde_json::to_string(&UpdateSource::McpCommand).unwrap(),
        "\"mcp_command\""
    );
    assert_eq!(
        serde_json::to_string(&UpdateSource::Manual).unwrap(),
        "\"manual\""
    );
}

#[test]
fn update_source_deserializes_from_snake_case() {
    let parsed: UpdateSource = serde_json::from_str("\"focus_lost\"").unwrap();
    assert!(matches!(parsed, UpdateSource::FocusLost));
}

#[test]
fn position_serializes_as_object() {
    let p = Position { line: 17, column: 5 };
    let s = serde_json::to_string(&p).unwrap();
    assert_eq!(s, r#"{"line":17,"column":5}"#);
}

#[test]
fn cursor_serializes_with_primary_flag() {
    let c = Cursor { primary: true, line: 17, column: 5 };
    let s = serde_json::to_string(&c).unwrap();
    assert_eq!(s, r#"{"primary":true,"line":17,"column":5}"#);
}

#[test]
fn selection_optional_text_omitted_when_none() {
    let sel = Selection {
        primary: true,
        start: Position { line: 1, column: 1 },
        end: Position { line: 2, column: 3 },
        byte_len: 17,
        text: None,
    };
    let s = serde_json::to_string(&sel).unwrap();
    assert!(!s.contains("\"text\""), "text field should be omitted: {}", s);
}

#[test]
fn active_round_trips_through_serde() {
    let a = Active {
        path: Some("src/main.rs".into()),
        path_abs: Some("/repo/src/main.rs".into()),
        language: Some("rust".into()),
        modified: false,
        line_count: 200,
        cursors: vec![Cursor { primary: true, line: 1, column: 1 }],
        selections: vec![],
        text: None,
    };
    let j = serde_json::to_value(&a).unwrap();
    let back: Active = serde_json::from_value(j).unwrap();
    assert_eq!(a.line_count, back.line_count);
    assert_eq!(a.cursors.len(), back.cursors.len());
}

#[test]
fn open_buffer_round_trips() {
    let b = OpenBuffer {
        path: Some("src/lib.rs".into()),
        language: Some("rust".into()),
        modified: true,
    };
    let j = serde_json::to_value(&b).unwrap();
    let back: OpenBuffer = serde_json::from_value(j).unwrap();
    assert_eq!(b.modified, back.modified);
}

use helix_context_schema::Instance;

#[test]
fn instance_round_trips() {
    let i = Instance {
        pid: 12345,
        socket_path: "/repo/.helix/control-12345.sock".into(),
        started_at: "2026-05-12T10:00:00Z".into(),
    };
    let j = serde_json::to_value(&i).unwrap();
    let back: Instance = serde_json::from_value(j).unwrap();
    assert_eq!(i.pid, back.pid);
    assert_eq!(i.socket_path, back.socket_path);
}
