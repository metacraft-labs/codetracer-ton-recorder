// Helper module imported by `imports_test.tolk` to exercise Tolk's
// cross-file `import "path";` resolution.  Real-world Tolk projects
// split contract logic across multiple files (utility helpers, type
// definitions, dispatch tables, …) and use `import` to stitch them
// together at compile time; the recorder must surface step events
// from each imported file under the correct source path so the
// frontend's source pane and the calltrace can route correctly.

struct Pair {
    a: int;
    b: int;
}

fun double(x: int): int {
    var d: int = x + x;
    return d;
}

fun pair_sum(p: Pair): int {
    var s: int = p.a + p.b;
    return s;
}
