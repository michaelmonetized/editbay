use crate::{Cancellation, Error, FloatTensor, Result};
use std::{collections::BTreeMap, io::Cursor, sync::Arc};
use tract_onnx::prelude::*;

pub(crate) struct TractGraph {
    model: InferenceModel,
    plan: Option<Arc<TypedRunnableModel>>,
    signature: Vec<(DatumType, Vec<usize>)>,
    names: Vec<String>,
    outputs: Vec<String>,
}

impl TractGraph {
    pub(crate) fn open(bytes: Vec<u8>) -> Result<Self> {
        let model = tract_onnx::onnx()
            .model_for_read(&mut Cursor::new(bytes))
            .map_err(error)?;
        let names = model
            .input_outlets()
            .map_err(error)?
            .iter()
            .map(|o| model.node(o.node).name.clone())
            .collect();
        let outputs = model
            .output_outlets()
            .map_err(error)?
            .iter()
            .map(|&o| {
                model
                    .outlet_label(o)
                    .map(str::to_owned)
                    .ok_or_else(|| Error::Invalid("reference output lacks a name".into()))
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            model,
            plan: None,
            signature: Vec::new(),
            names,
            outputs,
        })
    }

    pub(crate) fn run(
        &mut self,
        inputs: Vec<(&str, FloatTensor)>,
        labels: Option<(&str, Vec<usize>, Vec<i32>)>,
        cancel: &Cancellation,
    ) -> Result<BTreeMap<String, FloatTensor>> {
        cancel.check()?;
        let mut tensors = BTreeMap::new();
        for (name, input) in inputs {
            input.validate()?;
            if tensors
                .insert(
                    name,
                    Tensor::from_shape(&input.shape, &input.values).map_err(error)?,
                )
                .is_some()
            {
                return Err(Error::Invalid("duplicate reference input".into()));
            }
        }
        if let Some((name, shape, values)) = labels {
            if values.is_empty()
                || values.len() > 64
                || shape.iter().try_fold(1usize, |n, &s| n.checked_mul(s)) != Some(values.len())
            {
                return Err(Error::Invalid("invalid reference labels".into()));
            }
            tensors.insert(name, Tensor::from_shape(&shape, &values).map_err(error)?);
        }
        let mut values = TVec::new();
        for name in &self.names {
            values.push(
                tensors
                    .remove(name.as_str())
                    .ok_or_else(|| Error::Invalid(format!("missing reference {name}")))?
                    .into_tvalue(),
            );
        }
        if !tensors.is_empty() {
            return Err(Error::Invalid("unknown reference inputs".into()));
        }
        let signature: Vec<_> = values
            .iter()
            .map(|v| (v.datum_type(), v.shape().to_vec()))
            .collect();
        if self.plan.is_none() || self.signature != signature {
            let mut model = self.model.clone();
            for (i, input) in values.iter().enumerate() {
                model
                    .set_input_fact(
                        i,
                        InferenceFact::dt_shape(input.datum_type(), input.shape()),
                    )
                    .map_err(error)?;
            }
            self.plan = Some(
                model
                    .into_optimized()
                    .map_err(error)?
                    .into_runnable()
                    .map_err(error)?,
            );
            self.signature = signature;
        }
        cancel.check()?;
        let outputs = self.plan.as_ref().unwrap().run(values).map_err(error)?;
        cancel.check()?;
        let mut result = BTreeMap::new();
        for (name, output) in self.outputs.iter().zip(outputs) {
            let tensor = FloatTensor {
                shape: output.shape().to_vec(),
                values: output
                    .try_as_plain_ram()
                    .map_err(error)?
                    .as_slice::<f32>()
                    .map_err(error)?
                    .to_vec(),
            };
            tensor.validate()?;
            result.insert(name.clone(), tensor);
        }
        Ok(result)
    }
}

fn error(error: impl std::fmt::Display) -> Error {
    Error::Runtime(format!("independent Tract reference: {error}"))
}
