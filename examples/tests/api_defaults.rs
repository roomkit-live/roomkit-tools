//! The defaults of the structs a Python call's keywords stand for, as JSON,
//! for the public API parity gate: rustdoc shows no default value, so
//! `make api-parity` runs this test and compares each field's default with
//! the Python parameter of its name (`scripts/api_parity/defaults.py`).
//!
//! Each struct is built as a caller builds it, `Default` or `new` with its
//! required fields, then destructured whole: a field added to a struct fails
//! to compile here until it is dumped too. A struct whose fields are all
//! required, or that has none, has no default to dump. The gate fails on a
//! struct it compares that this dump lacks. `ROOMKIT_API_DEFAULTS` names the
//! file the dump is written to.
#![forbid(unsafe_code)]

use serde_json::{Map, Value};

/// The struct `$krate::$name` built by `$build`, destructured whole into the
/// fields listed after it, as its path and its fields' JSON.
macro_rules! dump {
    ($krate:ident :: $name:ident :: $build:ident ( $($arg:expr),* ) $(, $field:ident)* $(,)?) => {{
        let $krate::$name { $($field),* } = $krate::$name::$build($($arg),*);
        let mut fields = Map::new();
        $(fields.insert(stringify!($field).to_owned(), serde_json::to_value(&$field).unwrap());)*
        (concat!(stringify!($krate), "::", stringify!($name)).to_owned(), Value::Object(fields))
    }};
}

fn files() -> Vec<(String, Value)> {
    vec![
        dump!(
            roomkit_tools_fs::DiffInput::new("a", "b"),
            old_path,
            new_path,
            context_lines
        ),
        dump!(
            roomkit_tools_fs::DiffLimits::default(),
            max_file_bytes,
            max_lines,
            max_comparison_cells,
            max_output_bytes,
            max_context_lines
        ),
        dump!(
            roomkit_tools_fs::EditInput::new("a", "b", "c"),
            path,
            old_text,
            new_text,
            replace_all,
            expected_revision
        ),
        dump!(roomkit_tools_fs::EditLimits::default(), max_file_bytes),
        dump!(
            roomkit_tools_fs::GlobInput::new("*"),
            pattern,
            path,
            offset,
            limit
        ),
        dump!(
            roomkit_tools_fs::GlobLimits::default(),
            max_matches,
            max_output_bytes,
            max_scan_entries,
            max_depth
        ),
        dump!(
            roomkit_tools_fs::GrepInput::new("a"),
            pattern,
            path,
            include,
            literal,
            ignore_case,
            context_before,
            context_after,
            offset,
            limit
        ),
        dump!(
            roomkit_tools_fs::GrepLimits::default(),
            max_matches,
            max_output_bytes,
            max_scan_entries,
            max_depth,
            max_file_bytes,
            max_total_bytes,
            max_context_lines,
            max_line_bytes
        ),
        dump!(roomkit_tools_fs::ListInput::default(), path, offset, limit),
        dump!(
            roomkit_tools_fs::ListLimits::default(),
            max_entries,
            max_output_bytes,
            max_scan_entries
        ),
        dump!(
            roomkit_tools_fs::PatchInput::new(Vec::new()),
            changes,
            dry_run
        ),
        dump!(
            roomkit_tools_fs::PatchLimits::default(),
            max_changes,
            max_input_bytes,
            max_file_bytes,
            max_lines,
            max_comparison_cells,
            max_output_bytes,
            max_context_lines
        ),
        dump!(roomkit_tools_fs::ReadInput::new("a"), path, offset, limit),
        dump!(
            roomkit_tools_fs::ReadLimits::default(),
            max_file_bytes,
            max_output_bytes,
            max_lines
        ),
        dump!(
            roomkit_tools_fs::WriteInput::new("a", "b"),
            path,
            content,
            overwrite,
            expected_revision
        ),
        dump!(roomkit_tools_fs::WriteLimits::default(), max_file_bytes),
    ]
}

fn processes() -> Vec<(String, Value)> {
    vec![
        dump!(
            roomkit_tools_process::CommandEnv::default(),
            inherit_env,
            env
        ),
        dump!(
            roomkit_tools_process::OutputLimits::default(),
            max_archives,
            max_archives_per_owner,
            max_stream_bytes,
            max_total_bytes,
            max_read_bytes,
            write_timeout_ms
        ),
        dump!(
            roomkit_tools_process::OutputReadInput::new("a"),
            archive_id,
            offset,
            max_bytes
        ),
        dump!(
            roomkit_tools_process::ProcessLimits::default(),
            max_processes,
            max_processes_per_owner,
            max_command_bytes,
            max_output_bytes,
            max_read_bytes,
            max_write_bytes,
            write_timeout_ms,
            default_timeout_ms,
            max_timeout_ms
        ),
        dump!(
            roomkit_tools_process::ProcessReadInput::new("a"),
            process_id,
            stdout_offset,
            stderr_offset,
            max_bytes
        ),
        dump!(
            roomkit_tools_process::ProcessStartInput::new("a"),
            command,
            cwd,
            timeout_ms
        ),
        dump!(
            roomkit_tools_process::ProcessStatusInput::default(),
            process_id
        ),
        dump!(
            roomkit_tools_process::ProcessStopInput::new("a"),
            process_id,
            forget
        ),
        dump!(
            roomkit_tools_process::ProcessWriteInput::new("a"),
            process_id,
            text,
            close_stdin
        ),
        dump!(
            roomkit_tools_process::ShellInput::new("a"),
            command,
            cwd,
            timeout_ms
        ),
        dump!(
            roomkit_tools_process::ShellLimits::default(),
            max_command_bytes,
            max_output_bytes,
            default_timeout_ms,
            max_timeout_ms,
            keep_background_jobs
        ),
    ]
}

fn network() -> Vec<(String, Value)> {
    vec![
        dump!(
            roomkit_tools_web::WebFetchInput::new("a"),
            url,
            format,
            timeout_ms
        ),
        dump!(
            roomkit_tools_web::WebFetchLimits::default(),
            max_url_bytes,
            max_response_bytes,
            max_converted_bytes,
            max_output_bytes,
            max_redirects,
            default_timeout_ms,
            max_timeout_ms
        ),
        dump!(
            roomkit_tools_web::WebFetchPolicy::default(),
            allowed_origins,
            private_origins
        ),
        dump!(
            roomkit_tools_search::OpenAISearchConfig::new("a"),
            model,
            max_output_tokens,
            max_tool_calls
        ),
        dump!(
            roomkit_tools_search::SearchHttpConfig::default(),
            endpoint,
            allowed_origins,
            private_origins,
            max_response_bytes
        ),
        dump!(
            roomkit_tools_search::WebSearchInput::new("a"),
            query,
            limit,
            timeout_ms
        ),
        dump!(
            roomkit_tools_search::SearchLimits::default(),
            max_query_bytes,
            max_results,
            max_content_bytes,
            max_output_bytes,
            default_timeout_ms,
            max_timeout_ms
        ),
    ]
}

fn media() -> Vec<(String, Value)> {
    vec![
        dump!(
            roomkit_tools_code::CodeOutlineInput::new("a"),
            path,
            language,
            offset,
            limit,
            expected_revision,
            timeout_ms
        ),
        dump!(
            roomkit_tools_code::CodeOutlineLimits::default(),
            max_file_bytes,
            max_output_bytes,
            max_symbols,
            max_scan_symbols,
            max_scan_nodes,
            max_depth,
            max_signature_bytes,
            default_timeout_ms,
            max_timeout_ms
        ),
        dump!(
            roomkit_tools_image::ViewImageInput::new("a"),
            path,
            timeout_ms
        ),
        dump!(
            roomkit_tools_image::ViewImageLimits::default(),
            max_file_bytes,
            max_dimension,
            max_pixels,
            max_decode_bytes,
            max_edge,
            max_image_bytes,
            default_timeout_ms,
            max_timeout_ms
        ),
        dump!(
            roomkit_tools_ocr::OcrInput::new("a"),
            path,
            rotation_degrees,
            expected_revision,
            timeout_ms
        ),
        dump!(
            roomkit_tools_ocr::OcrLimits::default(),
            max_file_bytes,
            max_dimension,
            max_pixels,
            max_decode_bytes,
            max_output_bytes,
            max_blocks,
            default_timeout_ms,
            max_timeout_ms
        ),
        dump!(
            roomkit_tools_ocr_rusto::RustoConfig::new("a"),
            models,
            model_name,
            threads,
            detection_max_edge
        ),
    ]
}

fn knowledge() -> Vec<(String, Value)> {
    vec![
        dump!(
            roomkit_tools_docs::DocsListInput::default(),
            offset,
            limit,
            expected_catalog_revision
        ),
        dump!(
            roomkit_tools_docs::DocsReadInput::new("a"),
            document_id,
            offset,
            limit,
            expected_revision
        ),
        dump!(
            roomkit_tools_docs::DocsSearchInput::new("a"),
            query,
            offset,
            limit,
            expected_catalog_revision
        ),
        dump!(
            roomkit_tools_docs::FileDocsLimits::default(),
            max_documents,
            max_document_bytes,
            max_total_bytes,
            max_entries,
            max_depth,
            max_total_lines,
            max_read_bytes
        ),
        dump!(
            roomkit_tools_plans::MemoryPlanLimits::default(),
            max_plans,
            max_plans_per_scope
        ),
        dump!(
            roomkit_tools_plans::PlanUpdateInput::new("a", 1),
            plan_id,
            expected_revision,
            title,
            objective,
            steps
        ),
        dump!(roomkit_tools_agents::TaskStatusInput::default(), task_id),
    ]
}

#[test]
fn every_keyword_struct_dumps_its_defaults() {
    let dumped: Vec<(String, Value)> = [files(), processes(), network(), media(), knowledge()]
        .into_iter()
        .flatten()
        .collect();
    let defaults: Map<String, Value> = dumped.iter().cloned().collect();
    assert_eq!(defaults.len(), dumped.len(), "a struct is dumped twice");
    if let Ok(path) = std::env::var("ROOMKIT_API_DEFAULTS") {
        let text = serde_json::to_string_pretty(&Value::Object(defaults)).unwrap();
        std::fs::write(path, text + "\n").unwrap();
    }
}
