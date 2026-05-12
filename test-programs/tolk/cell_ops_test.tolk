// Exercises TON-specific Cell / Slice / Builder operations.
//
// In Tolk, a `cell` is the primitive storage unit on the TON
// blockchain.  Builders construct cells (`beginCell()` ... `endCell()`)
// and slices read from them (`beginParse()` + `loadInt`/`loadUint`).
// Spec-compliant recording would surface:
//   * each builder/slice as a `ValueRecord` of the matching variant
//     (e.g. `Builder`, `Slice`, `Cell`),
//   * each `storeInt` / `loadInt` call as a step + var event with the
//     int payload,
//   * `get_data()` / `set_data()` as IO events tagged as TON storage
//     reads/writes.
//
// The current Tolk parser is line-oriented and does not understand
// any of these calls — they are opaque expression text and never
// surface as values.  The trace today contains only the integer
// let-bindings in zero-arg helpers (`encode_payload`,
// `decode_payload`, `compute`).  See RECORDER BUG notes in the
// matching test for the spec-compliant expectation.

fun encode_payload(): int {
    var payload: int = 42;
    var b = beginCell();
    var b2 = b.storeInt(payload, 32);
    var c: cell = b2.endCell();
    var encoded_marker: int = 1;
    return encoded_marker;
}

fun decode_payload(): int {
    var c: cell = beginCell().storeInt(42, 32).endCell();
    var s = c.beginParse();
    var loaded: int = s.loadInt(32);
    var decoded_marker: int = 2;
    return decoded_marker;
}

fun storage_roundtrip(): int {
    var b = beginCell().storeInt(99, 32);
    var c: cell = b.endCell();
    set_data(c);
    var got: cell = get_data();
    var s = got.beginParse();
    var loaded: int = s.loadInt(32);
    var roundtrip_marker: int = 3;
    return roundtrip_marker;
}

fun compute(): int {
    var encoded: int = encode_payload();
    var decoded: int = decode_payload();
    var rt: int = storage_roundtrip();
    var combined: int = encoded + decoded + rt;
    return combined;
}

fun main(): int {
    return compute();
}
