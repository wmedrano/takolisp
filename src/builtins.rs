use crate::val::{Val, VmError, function::NativeFunction};

pub const PLUS_FN: NativeFunction = NativeFunction::new(plus_fn);
pub const MINUS_FN: NativeFunction = NativeFunction::new(minus_fn);
pub const LESS_THAN_FN: NativeFunction = NativeFunction::new(less_than_fn);

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

fn number_as_float(value: &Val) -> Result<f64, VmError> {
    match value {
        Val::Int(value) => Ok(*value as f64),
        Val::Float(value) => Ok(*value),
        _ => Err(VmError::InvalidType {
            expected: "number",
            actual: value.clone(),
        }),
    }
}

fn minus_fn(args: &[Val]) -> Result<Val, VmError> {
    let (first, rest) = args.split_first().ok_or(VmError::MissingArgument)?;
    if rest.is_empty() {
        return match first {
            Val::Int(value) => value
                .checked_neg()
                .map(Val::Int)
                .ok_or(VmError::IntegerOverflow),
            value => Ok(Val::Float(-number_as_float(value)?)),
        };
    }

    let mut total = first.clone();
    for arg in rest {
        total = match (&total, arg) {
            (Val::Int(lhs), Val::Int(rhs)) => lhs
                .checked_sub(*rhs)
                .map(Val::Int)
                .ok_or(VmError::IntegerOverflow)?,
            (lhs, rhs) => Val::Float(number_as_float(lhs)? - number_as_float(rhs)?),
        };
    }
    Ok(total)
}

fn less_than_fn(args: &[Val]) -> Result<Val, VmError> {
    let (mut previous, rest) = args.split_first().ok_or(VmError::MissingArgument)?;
    let mut previous_float = number_as_float(previous)?;
    let mut increasing = true;

    for current in rest {
        let current_float = number_as_float(current)?;
        increasing &= match (previous, current) {
            (Val::Int(lhs), Val::Int(rhs)) => lhs < rhs,
            _ => previous_float < current_float,
        };
        previous = current;
        previous_float = current_float;
    }

    Ok(Val::Bool(increasing))
}
