//! Every optimization level agrees with `-O0` under the interpreter, on
//! output, final tape, and fault. `-O3` joins once its passes exist.

use bfr::{
    CellWidth, Config, Dialect, EofBehavior, FaultCode, OptLevel, RuntimeError, Session, Step,
};

const LEVELS: [OptLevel; 2] = [OptLevel::O1, OptLevel::O2];

/// Enough for every program here. Running out means a program hung, and
/// the test fails loudly instead.
const FUEL: u64 = 10_000_000;

/// Name, source, and input.
const CORPUS: &[(&str, &str, &[u8])] = &[
    ("multiply", "+++[>++++<-]>.", b""),
    (
        "hello",
        "++++++++[>++++[>++>+++>+++>+<<<<-]>+>+>->>+[<]<-]>>.>---.+++++++..+++.>>.<-.<.+++.------.--------.>>+.>++.",
        b"",
    ),
    ("cat", ",[.,]", b"hello\n\0"),
    ("copy through a temp", "+++++[->+>+<<]>>[-<<+>>]<<.", b""),
    ("comment loop", "[this is a comment]+++.", b""),
    ("reverse by scanning", ">,[>,]<[.<]", b"abc\0"),
    ("cleared accumulations", "++++[>++++<-]>[-]<[-]+.", b""),
    ("odd step drain", "++++++[--->+<]>.", b""),
    ("even step loop", "++[--]+.", b""),
    ("input at eof", ",.,.", b"x"),
    (
        "wrap seen by a loop test",
        "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++[>+<[-]]>.",
        b"",
    ),
    ("a shell inside a loop", "+++[>+++[>+<-]<-]>>.", b""),
];

/// Name and source of programs that fault.
const FAULTING: &[(&str, &str)] = &[
    ("underflow on a write", "<+"),
    ("underflow on a loop test", "<[-]"),
    ("underflow through a scan", "+[<]"),
    ("underflow on a kept loop", "<[+.]"),
    ("overflow on a walk", "+[>+]"),
];

fn dialects() -> Vec<Dialect> {
    let mut out = Vec::new();
    for cell_width in [CellWidth::U8, CellWidth::U16, CellWidth::U32] {
        for eof in [
            EofBehavior::Zero,
            EofBehavior::Unchanged,
            EofBehavior::MinusOne,
        ] {
            out.push(Dialect {
                cell_width,
                eof,
                tape_cells: 256,
                ..Dialect::default()
            });
        }
    }
    out
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Done {
        output: Vec<u8>,
        ptr: isize,
        tape: Vec<u32>,
    },
    Fault(RuntimeError),
}

fn run(src: &str, config: &Config, input: &[u8]) -> Outcome {
    let (program, stats) = bfr::compile_to_ir(src, config).expect("parses");
    assert!(
        stats.reached_fixed_point(&config.pipeline),
        "the pipeline did not settle on {src}"
    );
    let mut session = Session::new(program, config);
    session.feed_input(input);
    session.close_input();
    loop {
        match session.run(u64::MAX) {
            Step::Yielded => {}
            Step::Done => {
                let tape = session.tape();
                let tape = (0..tape.len_cells())
                    .map(|i| tape.get(i).expect("in range"))
                    .collect();
                return Outcome::Done {
                    output: session.take_output(),
                    ptr: session.ptr(),
                    tape,
                };
            }
            Step::Fault(fault) => return Outcome::Fault(fault),
            other => panic!("{other:?} with input closed and no breakpoints"),
        }
    }
}

#[test]
fn the_corpus_runs_as_expected_at_o0() {
    let config = Config::new(OptLevel::O0);
    let hello = bfr::run_to_vec(CORPUS[1].1, &config, b"").expect("runs");
    assert_eq!(hello, b"Hello World!\n");
    let multiply = bfr::run_to_vec(CORPUS[0].1, &config, b"").expect("runs");
    assert_eq!(multiply, [12]);
}

#[test]
fn levels_agree_under_interpretation() {
    for &(name, src, input) in CORPUS {
        for dialect in dialects() {
            let o0 = Config::new(OptLevel::O0)
                .with_dialect(dialect)
                .with_fuel(FUEL);
            let reference = run(src, &o0, input);
            assert!(
                matches!(reference, Outcome::Done { .. }),
                "{name} at O0 under {dialect:?}: {reference:?}"
            );
            for level in LEVELS {
                let config = Config::new(level).with_dialect(dialect).with_fuel(FUEL);
                assert_eq!(
                    run(src, &config, input),
                    reference,
                    "{name} at {level} under {dialect:?}"
                );
            }
        }
    }
}

#[test]
fn bounds_violations_fault_alike() {
    for &(name, src) in FAULTING {
        for dialect in dialects() {
            let o0 = Config::new(OptLevel::O0)
                .with_dialect(dialect)
                .with_fuel(FUEL);
            let reference = run(src, &o0, b"");
            assert!(
                matches!(reference, Outcome::Fault(RuntimeError { code, .. }) if code != FaultCode::OutOfFuel),
                "{name} at O0 under {dialect:?}: {reference:?}"
            );
            for level in LEVELS {
                let config = Config::new(level).with_dialect(dialect).with_fuel(FUEL);
                assert_eq!(
                    run(src, &config, b""),
                    reference,
                    "{name} at {level} under {dialect:?}"
                );
            }
        }
    }
}

#[test]
fn fuel_terminates_infinite_loops() {
    for level in [OptLevel::O0, OptLevel::O1, OptLevel::O2] {
        let config = Config::new(level).with_fuel(10_000);
        match run("+[]", &config, b"") {
            Outcome::Fault(RuntimeError { code, .. }) => assert_eq!(code, FaultCode::OutOfFuel),
            other => panic!("{other:?} at {level}"),
        }
    }
}
