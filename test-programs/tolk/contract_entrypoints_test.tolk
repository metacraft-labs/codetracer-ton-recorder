// Exercises Tolk's actual on-chain entry-point hooks:
// `onInternalMessage(...)` (invoked when the contract receives an
// internal message from another contract) and `onExternalMessage(...)`
// (invoked when the contract receives an external message from outside
// the chain).  Real-world TON contracts do NOT declare a `main()` —
// the TVM dispatches incoming messages directly to whichever of these
// hooks matches the message kind.
//
// Pre-M10, the recorder hard-failed if `main()` wasn't present
// (`evaluate_program` short-circuited with `eyre!("no main function")`).
// Spec-compliant recording recognises both hooks as entry points: the
// first declared hook merges into <toplevel> at depth 0 (same
// rationale as the legacy `main()` arm), and any subsequent hook runs
// as a normal nested call so its body opens a fresh call_entry /
// call_exit pair.
//
// The recorder feeds synthetic `int(0)` actuals for every formal so
// the body executes and surfaces step + var events.  Bigger fixtures
// with richer entry-point signatures (Cells / Slices / message
// payloads) will extend this stub once the recorder learns to
// synthesise representative payloads.

fun handle_internal(amount: int): int {
    var doubled: int = amount + amount;
    var ack: int = doubled + 1;
    return ack;
}

fun handle_external(seq: int): int {
    var bumped: int = seq + 10;
    return bumped;
}

fun onInternalMessage(amount: int): int {
    var processed: int = handle_internal(amount);
    return processed;
}

fun onExternalMessage(seq: int): int {
    var processed: int = handle_external(seq);
    return processed;
}
