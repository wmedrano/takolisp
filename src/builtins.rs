use crate::val::{Val, VmError, function::NativeFunction};

pub const PLUS_FN: NativeFunction = NativeFunction::new(plus_fn);

fn plus_fn(args: &[Val]) -> Result<Val, VmError> {
    let mut int_sum = 0;
    let mut float_sum = 0.0;
    let mut has_float = false;
    for arg in args {
        match arg {
            Val::Int(x) => int_sum += *x,
            Val::Float(x) => {
                float_sum += *x;
                has_float = true;
            }
            _ => {
                return Err(VmError::InvalidType {
                    expected: "number",
                    actual: arg.clone(),
                });
            }
        }
    }
    let sum = if has_float {
        Val::Float(int_sum as f64 + float_sum)
    } else {
        Val::Int(int_sum)
    };
    Ok(sum)
}
