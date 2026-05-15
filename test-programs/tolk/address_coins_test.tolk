// Exercises Tolk's TON-specific domain types `address` and `coin`.
// Real-world TON contracts route value-bearing logic through these
// types pervasively: every `onInternalMessage` handler binds the
// caller's coin balance, the message value, and the message origin's
// address before any contract-specific dispatch happens.
//
// Pre-this-fixture the recorder treated every type other than `int` /
// `bool` / `<Struct>` as an unrecognised tag and silently lifted
// `loadAddress()` to a plain `Int` ValueRecord regardless of the
// declared LHS type.  Spec-compliant recording surfaces:
//   * `address` as a structured `ValueRecord::Struct` carrying two
//     fields (`workchain: int`, `hash: int`) — frontend / address-
//     decoder consumers can render the pair as `<wc>:<hex-hash>`,
//   * `coin` as a tagged `ValueRecord::Int` whose `type_id` resolves
//     to the lazily-registered `coin` slot (NOT the generic `int`
//     type_id) so consumers can route on the type before formatting
//     the value as nanoton / TON.
//
// The handler's body seeds the message slice with a single int
// payload (the sender address hash) and then reads it back via
// `slice.loadAddress()`; the binding's `: address` annotation drives
// the recorder's int-to-Address lift so the resulting trace carries
// a real Struct value (workchain=0, hash=51966).

fun handle_message(myBalance: coin, msgValue: coin, msgFull: cell, msgBody: slice): int {
    var sender_slice: slice = msgFull.beginParse();
    var sender: address = sender_slice.loadAddress();
    var combined: int = myBalance + msgValue;
    return combined;
}

fun onInternalMessage(myBalance: coin, msgValue: coin, msgFull: cell, msgBody: slice): int {
    var processed: int = handle_message(myBalance, msgValue, msgFull, msgBody);
    return processed;
}
