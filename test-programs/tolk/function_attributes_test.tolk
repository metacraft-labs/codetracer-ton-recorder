// Exercises Tolk's function-attribute syntax: `@method_id(N)`,
// `@inline`, `@inline_ref`, and `@pure` decorators that appear on
// the line(s) preceding a `fun` declaration.  Real-world Tolk
// contracts use these pervasively (every TON jetton/wallet exports
// `get_*` methods via `@method_id(N)`; hot helpers carry `@inline`),
// so a recorder that drops the helper because of the attribute line
// is a strict regression.
//
// Pre-this-fixture the hand-rolled parser only recognised lines
// starting with `fun ` as function declarations; an `@<attr>` line
// preceding the `fun` line was treated as a statement and the
// function itself was either skipped (causing call_entry to misfire)
// or — worse — interpreted as the body of the previous function.
//
// Spec-compliant recording: each decorated helper must surface as a
// regular `Function` entry (registered with the right name) and
// produce normal Call/Return events when invoked from `compute()`.
// `@method_id(99)` MUST NOT cause `owner_id` to be dropped or
// renamed; the canonical method id flows through as metadata once
// the recorder gains a dedicated channel for it (today it is simply
// preserved on the function struct so the test can pin function
// presence).

@method_id(99)
fun owner_id(): int {
    return 42;
}

@inline
fun add(a: int, b: int): int {
    return a + b;
}

@inline_ref
fun store_zero(): int {
    return 0;
}

@pure
fun is_positive(x: int): int {
    if (x > 0) {
        return 1;
    }
    return 0;
}

fun compute(): int {
    var oid: int = owner_id();
    var sum: int = add(2, 3);
    var zero: int = store_zero();
    var pos: int = is_positive(7);
    var grand: int = oid + sum + zero + pos;
    return grand;
}

fun main(): int {
    return compute();
}
