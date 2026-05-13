// Exercises Tolk's positional function-argument passing.
//
// Pre-M10, the recorder's hand-rolled `parse_function_call` only
// recognised bare zero-arg call sites (`compute()`); every multi-arg
// call was silently dropped on the floor (the call expression
// short-circuited to the int-only TVM path which has no notion of
// callees, and the caller's binding ended up unevaluated).
//
// Spec-compliant recording for this fixture surfaces:
//   * each callee binding (`a`, `b` in `add`; `n` in `square`; etc.)
//     as a step + var event — proving that formals are bound to the
//     resolved actual values at the call site,
//   * every helper's return value as a real Int decoded back into
//     the caller's binding,
//   * the four-deep call chain `compute -> chain_calls -> add ->
//     square` end-to-end, with each nested call carrying its own
//     argument list.
//
// This is the unlock fixture for every other multi-arg M10 program;
// without it `persistent_storage_test`, `contract_entrypoints_test`,
// `throw_unless_throw_if_test`, and `builder_refs_test` would all
// have to inline their helpers.

fun add(a: int, b: int): int {
    var sum: int = a + b;
    return sum;
}

fun square(n: int): int {
    var sq: int = n * n;
    return sq;
}

fun chain_calls(seed: int): int {
    var doubled: int = add(seed, seed);
    var squared: int = square(doubled);
    return squared;
}

fun compute(): int {
    var base: int = 3;
    var sum: int = add(base, 4);
    var sq: int = square(base);
    var chained: int = chain_calls(base);
    var combined: int = sum + sq + chained;
    return combined;
}

fun main(): int {
    return compute();
}
