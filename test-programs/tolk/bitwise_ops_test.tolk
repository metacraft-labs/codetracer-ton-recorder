// Exercises Tolk's bitwise operator coverage: `&`, `|`, `^`, `~`,
// `<<`, `>>`.  Pre-this-fixture the recorder's TVM expression
// pipeline only knew about `+ - * / %` and the comparison family
// (`==`, `!=`, `<`, `>`, `<=`, `>=`).  Bitwise operators are
// pervasive in real-world TON contracts (flag packing, address
// masking, bit-level cell parsing), so a recorder that silently
// drops them is a strict regression.
//
// Spec-compliant recording: each result variable lands as a real
// `ValueRecord::Int` carrying the TVM-computed answer.  The chained
// `combined` expression `(c | d) ^ (e & f)` exercises the
// precedence relationships between `|`, `^`, and `&` end-to-end.

fun bitwise(): int {
    var a: int = 240;
    var b: int = 15;
    var c: int = a & b;
    var d: int = a | b;
    var e: int = a ^ b;
    var f: int = ~a;
    var g: int = a << 4;
    var h: int = a >> 2;
    var combined: int = (c | d) ^ (e & f);
    return combined;
}

fun main(): int {
    return bitwise();
}
