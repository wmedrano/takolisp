# Takolisp

A small Lisp interpreter written in Rust.

## Building

```sh
cargo build
```

## Running a script

```sh
cargo run --bin takolisp -- example.lisp
```

## Interactive REPL

```sh
cargo run --bin takolisp
```

The result of each line is bound to `$1`, `$2`, and so on in the REPL module.

Force colored output with `--color`.

## Library

The interpreter is usable as a library through `takolisp::Vm`. The REPL is also
available as a library API through `takolisp::repl::Repl`.

Both the binary and `takolisp::repl` are behind the default `repl` feature. Build
the interpreter without it (and without the readline dependency) with:

```sh
cargo build --no-default-features
```

## Tests and benchmarks

```sh
cargo test
cargo bench
```
