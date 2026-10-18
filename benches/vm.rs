use criterion::{Criterion, black_box, criterion_group, criterion_main};
use takolisp::{Val, Vm, symbol::Symbol};

fn eval_closure_close(c: &mut Criterion) {
    let mut vm = Vm::new();
    vm.eval_str(
        vm.global_module(),
        "(define thunk (lambda () (((lambda (x) (lambda () x)) 42))))",
    )
    .unwrap();
    let thunk = vm.get_global(Symbol::must_inline("thunk"));
    c.bench_function("eval_closure_close", |b| {
        b.iter(|| black_box(vm.eval(thunk, [])))
    });
}

fn eval_boxed_upvalue_call(c: &mut Criterion) {
    let mut vm = Vm::new();
    vm.eval_str(
        vm.global_module(),
        r#"
(define (adder base step)
  (lambda (n) (+ base step n)))"#,
    )
    .unwrap();
    let make_adder = vm.get_global(Symbol::must_inline("adder"));
    let adder = vm.eval(make_adder, [Val::Int(1), Val::Int(2)]).unwrap();
    c.bench_function("eval_boxed_upvalue_call", |b| {
        b.iter(|| black_box(vm.eval(adder, [Val::Int(3)])))
    });
}

fn eval_naive_fib(c: &mut Criterion) {
    let mut vm = Vm::new();
    let module = vm.global_module();
    vm.eval_str(
        module,
        r#"
(define (fib n)
  (if (< n 2)
    n
    (+ (fib (- n 1))
       (fib (- n 2)))))
"#,
    )
    .unwrap();
    let fib = vm.get_global(Symbol::must_inline("fib"));
    c.bench_function("eval_fib", |b| {
        b.iter(|| black_box(vm.eval(fib, [Val::Int(18)])))
    });
}

criterion_group!(
    benches,
    eval_closure_close,
    eval_boxed_upvalue_call,
    eval_naive_fib
);
criterion_main!(benches);
