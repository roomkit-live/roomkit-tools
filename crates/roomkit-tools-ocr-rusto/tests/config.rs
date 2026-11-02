//! The models directory and the defaults Python's `RustoOcr` takes.
use roomkit_tools_core::ToolError;
use roomkit_tools_ocr_rusto::{RustoConfig, RustoOcr};

#[test]
fn a_models_directory_takes_python_defaults() {
    let config = RustoConfig::new("models");
    assert_eq!(config.models, std::path::Path::new("models"));
    assert_eq!(config.model_name, "ppocrv6");
    assert_eq!(config.threads, 2);
    assert_eq!(config.detection_max_edge, 960);
}

#[test]
fn models_are_read_from_their_names_in_the_directory() {
    let models = tempfile::tempdir().unwrap();
    assert!(matches!(
        RustoOcr::load(RustoConfig::new(models.path())),
        Err(ToolError::Io { .. })
    ));
    for name in ["det.onnx", "rec.onnx", "dict.txt"] {
        std::fs::write(models.path().join(name), "not a model").unwrap();
    }
    // Every file is found and read; only the model bytes are refused.
    assert!(matches!(
        RustoOcr::load(RustoConfig::new(models.path())),
        Err(ToolError::UnsupportedResource(_))
    ));
}
