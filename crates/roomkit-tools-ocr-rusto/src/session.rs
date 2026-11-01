//! Use explicit `RTen` pools rather than the upstream global thread default.
use std::{collections::HashMap, sync::Arc};

use ndarray::ArrayD;
use rten::{Model, RunOptions, ThreadPool, Value};
use rten_tensor::{Tensor, prelude::*};
use rusto::engine::{EngineError, InferenceSession};

pub(crate) struct Session {
    pub model: Model,
    pub pool: Arc<ThreadPool>,
}

impl Session {
    fn infer(
        &self,
        inputs: HashMap<String, ArrayD<f32>>,
    ) -> Result<HashMap<String, ArrayD<f32>>, EngineError> {
        let mut owned = Vec::new();
        for (name, input) in inputs {
            let id = self
                .model
                .find_node(&name)
                .ok_or_else(|| EngineError::OutputError("missing model input".into()))?;
            owned.push((
                id,
                Tensor::from_data(input.shape(), input.iter().copied().collect::<Vec<f32>>()),
            ));
        }
        let outputs = self.model.run(
            owned
                .iter()
                .map(|(id, tensor)| (*id, tensor.view().into()))
                .collect(),
            self.model.output_ids(),
            Some(RunOptions::default().with_thread_pool(Some(Arc::clone(&self.pool)))),
        )?;
        self.model
            .output_ids()
            .iter()
            .zip(outputs)
            .map(|(id, value)| {
                let name = self
                    .model
                    .node_info(*id)
                    .and_then(|node| node.name())
                    .ok_or_else(|| EngineError::OutputError("unnamed model output".into()))?;
                let Value::FloatTensor(tensor) = value else {
                    return Err(EngineError::OutputError(
                        "OCR model must return floating point outputs".into(),
                    ));
                };
                Ok((
                    name.to_owned(),
                    ArrayD::from_shape_vec(tensor.shape(), tensor.iter().copied().collect())?,
                ))
            })
            .collect()
    }
}

/// Recognizer session that supplies the already validated vocabulary in memory,
/// so `RustO` never opens the dictionary path itself.
pub(crate) struct Vocabulary<S> {
    pub session: S,
    pub characters: Vec<String>,
}

const CHARACTER_KEY: &str = "character";

impl<S: InferenceSession> InferenceSession for Vocabulary<S> {
    fn run(&mut self, input: ArrayD<f32>) -> Result<ArrayD<f32>, EngineError> {
        self.session.run(input)
    }

    fn run_all(&mut self, input: ArrayD<f32>) -> Result<HashMap<String, ArrayD<f32>>, EngineError> {
        self.session.run_all(input)
    }

    fn run_with_inputs(
        &mut self,
        inputs: HashMap<String, ArrayD<f32>>,
    ) -> Result<HashMap<String, ArrayD<f32>>, EngineError> {
        self.session.run_with_inputs(inputs)
    }

    fn get_character_list(&self, key: &str) -> Option<Vec<String>> {
        (key == CHARACTER_KEY).then(|| self.characters.clone())
    }

    fn have_key(&self, key: &str) -> bool {
        key == CHARACTER_KEY
    }
}

impl InferenceSession for Session {
    fn run(&mut self, input: ArrayD<f32>) -> Result<ArrayD<f32>, EngineError> {
        let output = self
            .model
            .output_ids()
            .first()
            .and_then(|id| self.model.node_info(*id))
            .and_then(|n| n.name())
            .ok_or_else(|| EngineError::OutputError("missing model output".into()))?
            .to_owned();
        self.run_all(input)?
            .remove(&output)
            .ok_or_else(|| EngineError::OutputError("missing inference output".into()))
    }

    fn run_all(&mut self, input: ArrayD<f32>) -> Result<HashMap<String, ArrayD<f32>>, EngineError> {
        let name = self
            .model
            .input_ids()
            .first()
            .and_then(|id| self.model.node_info(*id))
            .and_then(|n| n.name())
            .ok_or_else(|| EngineError::OutputError("missing model input".into()))?;
        self.infer(HashMap::from([(name.to_owned(), input)]))
    }

    fn run_with_inputs(
        &mut self,
        inputs: HashMap<String, ArrayD<f32>>,
    ) -> Result<HashMap<String, ArrayD<f32>>, EngineError> {
        self.infer(inputs)
    }
}
