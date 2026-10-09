#![forbid(unsafe_code)]
use serde::{Deserialize, Serialize};

include!(concat!(env!("OUT_DIR"), "/user_summary.rs"));

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn authored_schema() -> Value {
        serde_json::from_str(include_str!(
            "../../../contracts/rpc/version/authored.schema.json"
        ))
        .expect("independently authored JSON Schema A parses")
    }

    #[test]
    fn absent_optional_user_fields_are_omitted_not_serialized_as_null() {
        let user = UserSummary {
            id: "user-123".to_owned(),
            user_name: None,
            display_name: None,
            active: None,
        };
        let observed =
            serde_json::to_value(&user).expect("actual source-derived Rust serialization");
        assert_eq!(observed, json!({"id": "user-123"}));
        for response in ["FindUsersResponse", "FindUserByIdResponse"] {
            let schema = authored_schema();
            let item = if response == "FindUsersResponse" {
                &schema["$defs"][response]["properties"]["results"]["items"]
            } else {
                &schema["$defs"][response]["properties"]["result"]
            };
            assert_eq!(item["required"], json!(["id"]));
            for field in ["user_name", "display_name", "active"] {
                assert!(
                    !item["required"].as_array().unwrap().contains(&json!(field)),
                    "{response} incorrectly requires optional {field}"
                );
                let kind = if field == "active" {
                    "boolean"
                } else {
                    "string"
                };
                assert_eq!(item["properties"][field]["type"], json!(kind));
            }
        }
    }

    #[test]
    fn populated_fields_keep_the_authored_user_wire_names() {
        let user = UserSummary {
            id: "user-123".to_owned(),
            user_name: Some("alice".to_owned()),
            display_name: Some("Alice".to_owned()),
            active: Some(true),
        };
        assert_eq!(
            serde_json::to_value(user).expect("production-derived serializer"),
            json!({
                "id": "user-123",
                "user_name": "alice",
                "display_name": "Alice",
                "active": true
            })
        );
    }
}
