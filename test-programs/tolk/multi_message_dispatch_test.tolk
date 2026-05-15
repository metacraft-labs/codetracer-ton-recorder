// @recorder:messages 0x01 0x02 0x03

// Exercises a Tolk contract that dispatches incoming internal messages
// on an op_code byte read from the message body.  Real-world TON
// contracts pervasively follow this idiom: the handler decodes the
// first 32-bit slice as an op-code, then routes through a chain of
// `if (op == 0x01) { ... } else if (op == 0x02) { ... }` branches
// (or, in newer Tolk, a `match` over a sum-typed dispatch table).
//
// Pre-this-fixture the recorder invoked the entry-point hook exactly
// once per record(...) call with a single synthetic int(0) actual
// per formal, so a fixture that exercised more than one branch
// silently surfaced only the `else` arm.  Spec-compliant recording
// supports a multi-message simulation: when the source declares the
// `// @recorder:messages <op0> <op1> ...` directive (the line
// directly above), the recorder re-invokes `onInternalMessage` once
// per op_code, threading each op_code into the synthesised
// `msgBody: slice` payload so the body's `msgBody.loadInt(32)`
// dispatch routes through a different branch on each call.
//
// The trace shape is one `call_entry` / `call_exit` pair per message
// (after the merged-into-toplevel first message), and per-branch
// step events that pin which arm fired.  See
// `test_multi_message_dispatch_test_via_ct_print_full` for the
// strict assertions.

fun handle_deposit(amount: int): int {
    var bumped: int = amount + 100;
    return bumped;
}

fun handle_withdraw(amount: int): int {
    var penalty: int = amount + 200;
    return penalty;
}

fun handle_query(amount: int): int {
    var marker: int = amount + 300;
    return marker;
}

fun onInternalMessage(myBalance: coin, msgValue: coin, msgFull: cell, msgBody: slice): int {
    var op: int = msgBody.loadInt(32);
    var processed: int = 0;
    if (op == 1) {
        processed = handle_deposit(op);
    } else if (op == 2) {
        processed = handle_withdraw(op);
    } else {
        processed = handle_query(op);
    }
    return processed;
}
