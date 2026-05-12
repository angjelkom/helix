use helix_context_schema::UpdateSource;

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
