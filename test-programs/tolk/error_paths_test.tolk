// Exercises Tolk's error / throw / assert surfaces.
//
// `safe_path()` runs a normal arithmetic chain so the recorder has a
// baseline of step + var events to emit before any failing path.
// `failing_path()` triggers `throw 7` after recording a couple of
// let-bindings.  `caught_path()` wraps a throwing callee in
// try/catch so the recorder can surface both the throw and its
// recovery.
//
// Spec-compliant recording would emit:
//   * step + var events for every binding before the throw,
//   * one io_event of `EventLogKind::Error` carrying the thrown code
//     (or assert message), and
//   * a recovery step inside the matching `catch` block.
//
// Today the Tolk parser does not recognise `throw`, `assert`,
// `try`, or `catch` as statements, so none of those events surface
// — see RECORDER BUG notes in the matching test.  This program is
// nevertheless committed as the canonical fixture so the assertions
// flip from "current shape" to "spec shape" automatically once the
// parser catches up.

fun safe_compute(): int {
    var a: int = 5;
    var b: int = 7;
    var c: int = a + b;
    return c;
}

fun failing_compute(): int {
    var seen_a: int = 1;
    var seen_b: int = 2;
    throw 7;
}

fun caught_compute(): int {
    var pre: int = 11;
    try {
        var inner_val: int = failing_compute();
        return inner_val;
    } catch (excno) {
        var recovered: int = pre + 100;
        return recovered;
    }
}

fun assert_compute(): int {
    var probe: int = 4;
    assert (probe > 0, 13);
    return probe;
}

fun compute(): int {
    var safe_val: int = safe_compute();
    var bumped: int = safe_val + 100;
    return bumped;
}

fun main(): int {
    return compute();
}
