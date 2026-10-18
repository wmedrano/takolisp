use crate::{
    error::RuntimeError, function::Arity, native_function::NativeFunction, symbol::Symbol,
    val::Val, vm::Vm,
};

pub fn register_builtins(vm: &mut Vm) {
    vm.set_global(Symbol::must_inline("+"), PLUS_FN.into());
    vm.set_global(Symbol::must_inline("-"), MINUS_FN.into());
    vm.set_global(Symbol::must_inline("*"), MULTIPLY_FN.into());
    vm.set_global(Symbol::must_inline("/"), DIVIDE_FN.into());
    vm.set_global(Symbol::must_inline("<"), LESS_THAN_FN.into());
    vm.set_global(Symbol::must_inline(">"), GREATER_THAN_FN.into());
    vm.set_global(Symbol::must_inline("<="), LESS_THAN_OR_EQUAL_FN.into());
    vm.set_global(Symbol::must_inline(">="), GREATER_THAN_OR_EQUAL_FN.into());
}

const PLUS_FN: &NativeFunction = &NativeFunction {
    name: "+",
    func: |context| {
        let args = context.args();
        let mut int_sum: i64 = 0;
        let mut float_sum: f64 = 0.0;
        let mut is_float = false;
        for arg in args {
            match *arg {
                Val::Int(i) => {
                    int_sum = int_sum.checked_add(i).ok_or(RuntimeError::Overflow)?;
                }
                Val::Float(f) => {
                    float_sum += f;
                    is_float = true;
                }
                found => {
                    return Err(RuntimeError::TypeError {
                        expected: "number",
                        found,
                    });
                }
            }
        }
        if is_float {
            Ok(Val::Float(float_sum + int_sum as f64))
        } else {
            Ok(Val::Int(int_sum))
        }
    },
};

const MINUS_FN: &NativeFunction = &NativeFunction {
    name: "-",
    func: |context| {
        let args = context.args();
        fn subtract_float(mut result: f64, args: &[Val]) -> Result<Val, RuntimeError> {
            for arg in args {
                match *arg {
                    Val::Int(i) => result -= i as f64,
                    Val::Float(f) => result -= f,
                    found => {
                        return Err(RuntimeError::TypeError {
                            expected: "number",
                            found,
                        });
                    }
                }
            }
            Ok(Val::Float(result))
        }

        let (first, rest) = match args {
            [Val::Int(i)] => {
                return i.checked_neg().map(Val::Int).ok_or(RuntimeError::Overflow);
            }
            [Val::Float(f)] => return Ok(Val::Float(-f)),
            [first, rest @ ..] => (first, rest),
            [] => {
                std::hint::cold_path();
                return Err(RuntimeError::ArityError {
                    expected: Arity::AtLeast(1),
                    found: args.len(),
                });
            }
        };

        let mut int_result = match *first {
            Val::Int(i) => i,
            Val::Float(f) => return subtract_float(f, rest),
            found => {
                return Err(RuntimeError::TypeError {
                    expected: "number",
                    found,
                });
            }
        };

        let mut args = rest.iter();
        while let Some(arg) = args.next() {
            match *arg {
                Val::Int(i) => {
                    int_result = int_result.checked_sub(i).ok_or(RuntimeError::Overflow)?;
                }
                Val::Float(f) => {
                    return subtract_float(int_result as f64 - f, args.as_slice());
                }
                found => {
                    return Err(RuntimeError::TypeError {
                        expected: "number",
                        found,
                    });
                }
            }
        }

        Ok(Val::Int(int_result))
    },
};

const MULTIPLY_FN: &NativeFunction = &NativeFunction {
    name: "*",
    func: |context| {
        let args = context.args();
        let mut int_product: i64 = 1;
        let mut float_product: f64 = 1.0;
        let mut is_float = false;
        for arg in args {
            match *arg {
                Val::Int(i) => {
                    int_product = int_product.checked_mul(i).ok_or(RuntimeError::Overflow)?;
                }
                Val::Float(f) => {
                    float_product *= f;
                    is_float = true;
                }
                found => {
                    return Err(RuntimeError::TypeError {
                        expected: "number",
                        found,
                    });
                }
            }
        }
        if is_float {
            Ok(Val::Float(float_product * int_product as f64))
        } else {
            Ok(Val::Int(int_product))
        }
    },
};

const DIVIDE_FN: &NativeFunction = &NativeFunction {
    name: "/",
    func: |context| divide_numbers(context.args()),
};

fn divide_numbers(args: &[Val]) -> Result<Val, RuntimeError> {
    let [first, rest @ ..] = args else {
        std::hint::cold_path();
        return Err(RuntimeError::ArityError {
            expected: Arity::AtLeast(1),
            found: args.len(),
        });
    };

    let mut result = if rest.is_empty() {
        1.0 / number_as_float(*first)?
    } else {
        number_as_float(*first)?
    };
    for arg in rest {
        result /= number_as_float(*arg)?;
    }
    Ok(Val::Float(result))
}

const LESS_THAN_FN: &NativeFunction = &NativeFunction {
    name: "<",
    func: |context| compare_numbers(context.args(), |a, b| a < b, |a, b| a < b),
};

const GREATER_THAN_FN: &NativeFunction = &NativeFunction {
    name: ">",
    func: |context| compare_numbers(context.args(), |a, b| a > b, |a, b| a > b),
};

const LESS_THAN_OR_EQUAL_FN: &NativeFunction = &NativeFunction {
    name: "<=",
    func: |context| compare_numbers(context.args(), |a, b| a <= b, |a, b| a <= b),
};

const GREATER_THAN_OR_EQUAL_FN: &NativeFunction = &NativeFunction {
    name: ">=",
    func: |context| compare_numbers(context.args(), |a, b| a >= b, |a, b| a >= b),
};

fn compare_numbers(
    args: &[Val],
    compare: fn(f64, f64) -> bool,
    compare_ints: fn(i64, i64) -> bool,
) -> Result<Val, RuntimeError> {
    let (mut previous, rest) = match args {
        [] => return Ok(true.into()),
        [first, rest @ ..] => (*first, rest),
    };
    for current in rest {
        let is_ordered = match (previous, *current) {
            (Val::Int(a), Val::Int(b)) => compare_ints(a, b),
            (a, b) => compare(number_as_float(a)?, number_as_float(b)?),
        };
        if !is_ordered {
            return Ok(false.into());
        }
        previous = *current;
    }
    Ok(true.into())
}

fn number_as_float(value: Val) -> Result<f64, RuntimeError> {
    match value {
        Val::Int(value) => Ok(value as f64),
        Val::Float(value) => Ok(value),
        found => Err(RuntimeError::TypeError {
            expected: "number",
            found,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // BUG (review): integer arithmetic in +, -, *, and negate is unchecked.
    // These currently panic with "attempt to add with overflow" (and friends)
    // in debug builds and wrap silently in release. They should return a
    // RuntimeError (e.g. Overflow) or promote to Float instead.
    #[test]
    fn add_rejects_integer_overflow() {
        let mut vm = Vm::new();

        assert!(
            vm.eval_str(vm.global_module(), "(+ 9223372036854775807 1)")
                .is_err()
        );
    }

    #[test]
    fn subtract_rejects_negating_i64_min() {
        let mut vm = Vm::new();

        assert!(
            vm.eval_str(vm.global_module(), "(- -9223372036854775808)")
                .is_err()
        );
    }

    #[test]
    fn multiply_rejects_integer_overflow() {
        let mut vm = Vm::new();

        assert!(
            vm.eval_str(vm.global_module(), "(* 9223372036854775807 2)")
                .is_err()
        );
    }

    #[test]
    fn compare_integers_beyond_float_precision() {
        let mut vm = Vm::new();

        assert_eq!(
            vm.eval_str(vm.global_module(), "(< 9007199254740992 9007199254740993)"),
            Ok(Val::Bool(true))
        );
    }

    #[test]
    fn subtract_negates_a_single_arg() {
        let mut vm = Vm::new();

        assert_eq!(vm.eval_str(vm.global_module(), "(- 5)"), Ok(Val::Int(-5)));
        assert_eq!(
            vm.eval_str(vm.global_module(), "(- 5.5)"),
            Ok(Val::Float(-5.5))
        );
    }

    #[test]
    fn subtract_applies_left_to_right_over_many_ints() {
        let mut vm = Vm::new();

        assert_eq!(
            vm.eval_str(vm.global_module(), "(- 20 3 2)"),
            Ok(Val::Int(15))
        );
    }

    #[test]
    fn subtract_promotes_to_float_if_any_arg_is_float() {
        let mut vm = Vm::new();

        assert_eq!(
            vm.eval_str(vm.global_module(), "(- 5.5 2)"),
            Ok(Val::Float(3.5))
        );
        assert_eq!(
            vm.eval_str(vm.global_module(), "(- 10 3.5)"),
            Ok(Val::Float(6.5))
        );
        assert_eq!(
            vm.eval_str(vm.global_module(), "(- 10 3 2.5)"),
            Ok(Val::Float(4.5))
        );
    }

    #[test]
    fn add_returns_zero_with_no_args() {
        let mut vm = Vm::new();

        assert_eq!(vm.eval_str(vm.global_module(), "(+)"), Ok(Val::Int(0)));
    }

    #[test]
    fn add_sums_any_number_of_ints() {
        let mut vm = Vm::new();

        assert_eq!(vm.eval_str(vm.global_module(), "(+ 5)"), Ok(Val::Int(5)));
        assert_eq!(
            vm.eval_str(vm.global_module(), "(+ 1 2 3)"),
            Ok(Val::Int(6))
        );
    }

    #[test]
    fn add_promotes_to_float_if_any_arg_is_float() {
        let mut vm = Vm::new();

        assert_eq!(
            vm.eval_str(vm.global_module(), "(+ 1 2.5)"),
            Ok(Val::Float(3.5))
        );
        assert_eq!(
            vm.eval_str(vm.global_module(), "(+ 1.5 2)"),
            Ok(Val::Float(3.5))
        );
        assert_eq!(
            vm.eval_str(vm.global_module(), "(+ 1 2 2.5)"),
            Ok(Val::Float(5.5))
        );
    }

    #[test]
    fn multiply_returns_one_with_no_args_and_promotes_floats() {
        let mut vm = Vm::new();

        assert_eq!(vm.eval_str(vm.global_module(), "(*)"), Ok(Val::Int(1)));
        assert_eq!(
            vm.eval_str(vm.global_module(), "(* 2 3 4)"),
            Ok(Val::Int(24))
        );
        assert_eq!(
            vm.eval_str(vm.global_module(), "(* 2 2.5)"),
            Ok(Val::Float(5.0))
        );
    }

    #[test]
    fn divide_always_returns_float_results() {
        let mut vm = Vm::new();

        assert_eq!(
            vm.eval_str(vm.global_module(), "(/ 20 2 5)"),
            Ok(Val::Float(2.0))
        );
        assert_eq!(
            vm.eval_str(vm.global_module(), "(/ 5 2)"),
            Ok(Val::Float(2.5))
        );
        assert_eq!(
            vm.eval_str(vm.global_module(), "(/ 5 2.0)"),
            Ok(Val::Float(2.5))
        );
    }

    #[test]
    fn divide_has_reciprocal_unary_form_and_ieee_zero_behavior() {
        let mut vm = Vm::new();

        assert_eq!(
            vm.eval_str(vm.global_module(), "(/ 4)"),
            Ok(Val::Float(0.25))
        );
        assert_eq!(
            vm.eval_str(vm.global_module(), "(/ 1 0)"),
            Ok(Val::Float(f64::INFINITY))
        );
        assert!(
            matches!(vm.eval_str(vm.global_module(), "(/ 0 0)"), Ok(Val::Float(value)) if value.is_nan())
        );
    }

    #[test]
    fn less_than_is_variadic_and_strict() {
        let mut vm = Vm::new();

        assert_eq!(vm.eval_str(vm.global_module(), "(<)"), Ok(Val::Bool(true)));
        assert_eq!(
            vm.eval_str(vm.global_module(), "(< 1)"),
            Ok(Val::Bool(true))
        );
        assert_eq!(
            vm.eval_str(vm.global_module(), "(< 1 2 3)"),
            Ok(Val::Bool(true))
        );
        assert_eq!(
            vm.eval_str(vm.global_module(), "(< 1 1 2)"),
            Ok(Val::Bool(false))
        );
        assert_eq!(
            vm.eval_str(vm.global_module(), "(< 1.0 2)"),
            Ok(Val::Bool(true))
        );
    }

    #[test]
    fn ordered_comparisons_are_variadic() {
        let mut vm = Vm::new();

        assert_eq!(
            vm.eval_str(vm.global_module(), "(> 3 2 1)"),
            Ok(Val::Bool(true))
        );
        assert_eq!(
            vm.eval_str(vm.global_module(), "(<= 1 1 2)"),
            Ok(Val::Bool(true))
        );
        assert_eq!(
            vm.eval_str(vm.global_module(), "(>= 2.5 2)"),
            Ok(Val::Bool(true))
        );
        assert_eq!(
            vm.eval_str(vm.global_module(), "(> 1 2)"),
            Ok(Val::Bool(false))
        );
    }
}
