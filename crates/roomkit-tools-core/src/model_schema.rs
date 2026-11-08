//! An input type's derived JSON Schema as a model reads it.
//!
//! The model reads every tool's parameters on every request. What only
//! describes the schema document or the Rust type is dropped; what the model
//! needs to write a valid call stays: types, bounds, required names,
//! descriptions, enumerations and defaults other than `null`.

use serde_json::{Map, Value};

/// The width formats schemars gives numbers. They describe the Rust type;
/// the bounds a model must respect stay as `minimum` and `maximum`.
const NUMBER_WIDTHS: [&str; 14] = [
    "int", "int8", "int16", "int32", "int64", "int128", "uint", "uint8", "uint16", "uint32",
    "uint64", "uint128", "float", "double",
];

/// Keywords whose value is one schema.
const ONE_SCHEMA: [&str; 7] = [
    "items",
    "additionalProperties",
    "not",
    "if",
    "then",
    "else",
    "contains",
];

/// Keywords whose value is a list of schemas.
const SCHEMA_LISTS: [&str; 4] = ["anyOf", "oneOf", "allOf", "prefixItems"];

/// Keywords whose value maps names to schemas: the names are not keywords.
const SCHEMA_MAPS: [&str; 3] = ["properties", "patternProperties", "$defs"];

/// `schema` without its `$schema`, its root `title` and `description` (the
/// Rust type's name and documentation, which the tool's own description
/// already covers), number width formats and `"default": null`.
///
/// Nullable types stay: a host that checks argument types accepts `null` for
/// an optional argument only when its type says so, and models send it.
pub(crate) fn for_model(mut schema: Value) -> Value {
    if let Value::Object(root) = &mut schema {
        for keyword in ["$schema", "title", "description"] {
            root.remove(keyword);
        }
    }
    strip(&mut schema);
    schema
}

/// Drop the type-only keywords of `schema` and of every schema it holds.
fn strip(schema: &mut Value) {
    let Value::Object(keywords) = schema else {
        return;
    };
    if keywords
        .get("format")
        .and_then(Value::as_str)
        .is_some_and(|format| NUMBER_WIDTHS.contains(&format))
    {
        keywords.remove("format");
    }
    if keywords.get("default") == Some(&Value::Null) {
        keywords.remove("default");
    }
    for (keyword, value) in keywords.iter_mut() {
        let keyword = keyword.as_str();
        if ONE_SCHEMA.contains(&keyword) {
            strip(value);
        } else if SCHEMA_LISTS.contains(&keyword) {
            value.as_array_mut().into_iter().flatten().for_each(strip);
        } else if SCHEMA_MAPS.contains(&keyword) {
            value
                .as_object_mut()
                .into_iter()
                .flat_map(Map::values_mut)
                .for_each(strip);
        }
    }
}

#[cfg(test)]
mod tests {
    use schemars::JsonSchema;
    use serde::Deserialize;
    use serde_json::json;

    use super::for_model;

    /// A step of the example input.
    #[derive(Deserialize, JsonSchema)]
    #[expect(dead_code, reason = "only the type's schema is read")]
    struct Step {
        /// How many times.
        count: u32,
        /// Its label, if any.
        format: Option<String>,
    }

    /// An example input, documented for Rust readers.
    #[derive(Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    #[expect(dead_code, reason = "only the type's schema is read")]
    struct Example {
        /// The title argument, a property named like a keyword.
        title: String,
        /// First line, from 1.
        #[schemars(range(min = 1))]
        #[serde(default = "one")]
        offset: u64,
        /// Line count, when bounded.
        #[serde(default)]
        limit: Option<u64>,
        /// The steps.
        steps: Vec<Step>,
        /// A ratio.
        ratio: f64,
    }

    fn one() -> u64 {
        1
    }

    #[test]
    fn a_model_reads_what_a_valid_call_needs_and_nothing_of_the_rust_type() {
        let derived = schemars::schema_for!(Example).to_value();
        assert_eq!(
            derived["title"], "Example",
            "the derived schema is the premise"
        );
        assert_eq!(derived["properties"]["limit"]["format"], "uint64");

        assert_eq!(
            for_model(derived),
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "The title argument, a property named like a keyword."
                    },
                    "offset": {
                        "type": "integer",
                        "minimum": 1,
                        "default": 1,
                        "description": "First line, from 1."
                    },
                    "limit": {
                        "type": ["integer", "null"],
                        "minimum": 0,
                        "description": "Line count, when bounded."
                    },
                    "steps": {
                        "type": "array",
                        "items": { "$ref": "#/$defs/Step" },
                        "description": "The steps."
                    },
                    "ratio": { "type": "number", "description": "A ratio." }
                },
                "required": ["title", "steps", "ratio"],
                "$defs": {
                    "Step": {
                        "type": "object",
                        "description": "A step of the example input.",
                        "properties": {
                            "count": {
                                "type": "integer",
                                "minimum": 0,
                                "description": "How many times."
                            },
                            "format": {
                                "type": ["string", "null"],
                                "description": "Its label, if any."
                            }
                        },
                        "required": ["count"]
                    }
                }
            })
        );
    }

    #[test]
    fn every_position_holding_a_schema_is_trimmed() {
        let width = || json!({ "type": "integer", "format": "uint64", "default": null });
        let trimmed = || json!({ "type": "integer" });
        for keyword in [
            "items",
            "additionalProperties",
            "not",
            "if",
            "then",
            "else",
            "contains",
        ] {
            let schema = json!({ "type": "object", keyword: width() });
            assert_eq!(
                for_model(schema),
                json!({ "type": "object", keyword: trimmed() }),
                "{keyword}"
            );
        }
        for keyword in ["anyOf", "oneOf", "allOf", "prefixItems"] {
            let schema = json!({ "type": "object", keyword: [width(), { "type": "null" }] });
            assert_eq!(
                for_model(schema),
                json!({ "type": "object", keyword: [trimmed(), { "type": "null" }] }),
                "{keyword}"
            );
        }
        for keyword in ["properties", "patternProperties", "$defs"] {
            let schema = json!({ "type": "object", keyword: { "name": width() } });
            assert_eq!(
                for_model(schema),
                json!({ "type": "object", keyword: { "name": trimmed() } }),
                "{keyword}"
            );
        }
        // One level down: a definition's variants.
        assert_eq!(
            for_model(json!({ "$defs": { "held": { "oneOf": [width()] } } })),
            json!({ "$defs": { "held": { "oneOf": [trimmed()] } } })
        );
    }

    #[test]
    fn formats_that_say_more_than_a_width_stay() {
        let schema = json!({ "type": "object", "properties": {
            "url": { "type": "string", "format": "uri" },
            "when": { "anyOf": [{ "type": "string", "format": "date-time" }, { "type": "null" }] }
        }});
        assert_eq!(for_model(schema.clone()), schema);
    }
}
