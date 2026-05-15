// Exercises Tolk's TVM context primitives backed by continuation
// register c7 (the contract-context cell): `now()` (block timestamp,
// `int`), `myBalance()` (contract balance, `coin`), `msgSender()`
// (incoming message origin address, `address`), `myAddress()`
// (this contract's address, `address`).
//
// Pre-this-fixture the recorder treated these as unknown calls and
// silently dropped the binding (the int-only TVM expression compiler
// has no symbol for them and the function-call dispatcher couldn't
// resolve them either).  Spec-compliant recording surfaces each
// primitive with the matching domain type AND a representative
// recorder-side TVM-context value (NOT a zero-default placeholder),
// so a downstream consumer sees the same shape it would see if the
// program were re-replayed against a live TVM with a populated
// continuation register c7.
//
// Fixture shape: a single handler `read_context()` reads each
// primitive into a local and returns a deterministic int summary so
// the trace pins both the binding shape AND the call-exit value.
//
// The recorder's deterministic c7 model is:
//   * now()       → 1_700_000_000
//   * myBalance() → 2_000_000_000  (2 TON in nanoton)
//   * msgSender() → workchain=0, hash=0xCAFE (51966)
//   * myAddress() → workchain=0, hash=0xBEEF (48879)

fun read_context(): int {
    var ts: int = now();
    var bal: coin = myBalance();
    var sender: address = msgSender();
    var here: address = myAddress();
    var summary: int = ts + bal;
    return summary;
}

fun main(): int {
    return read_context();
}
