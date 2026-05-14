// Exercises Tolk's `import "path";` cross-file resolution.  Real-
// world Tolk projects split contract logic across multiple files
// (utility helpers, type definitions, dispatch tables, …) and use
// `import` to stitch them together at compile time.
//
// Pre-this-fixture the recorder lumped every step under the entry
// file's source path because the parser only consumed one source
// file per `record(...)` invocation.  Spec-compliant recording must
// load each `import "path";` target, parse it, register its
// functions, and emit step events from the imported helpers under
// THEIR source path (not the entry file's), so the frontend's
// source pane lands on the right line.

import "imports_helpers.tolk";

fun compute(): int {
    var d: int = double(5);
    var p: Pair = Pair { a: 3, b: 4 };
    var s: int = pair_sum(p);
    var total: int = d + s;
    return total;
}

fun main(): int {
    return compute();
}
