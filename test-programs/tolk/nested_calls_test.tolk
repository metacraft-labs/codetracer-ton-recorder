// Four-deep zero-argument call chain:
//   main -> compute -> outer -> middle -> inner
//
// Every callee returns an int, and each level's body adds a fresh
// constant so the recorder's call sequence and per-level arithmetic
// can be pinned exactly.  Zero-argument calls are the well-behaved
// case for the present-day Tolk parser (`parse_function_call` only
// matches `name()` invocations) — this program is the "everything
// works" baseline against which other programs in this suite document
// recorder gaps.

fun inner(): int {
    var a: int = 1;
    var b: int = 2;
    var c: int = a + b;
    return c;
}

fun middle(): int {
    var x: int = inner();
    var y: int = x + 10;
    return y;
}

fun outer(): int {
    var p: int = middle();
    var q: int = p + 100;
    return q;
}

fun compute(): int {
    var result: int = outer();
    return result;
}

fun main(): int {
    return compute();
}
