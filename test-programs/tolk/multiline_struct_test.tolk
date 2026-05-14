// Exercises the recorder's multi-line struct- and tuple-literal
// parsing.  Pre-this-fixture the hand-rolled parser was strictly
// line-oriented: a struct literal had to fit on one source line, so
// `Point { x: 3, y: 4 }` parsed but the same value split across
// `Point {`, `    x: 3,`, `    y: 4`, `}` did not.  Real-world Tolk
// contracts routinely break long literals across lines for
// readability — this fixture pins the multi-line shape end-to-end so
// the equivalent single-line literal in `tuples_structs_test.tolk`
// keeps producing the same `ValueRecord::Struct` / `ValueRecord::Tuple`
// shape regardless of source-level line breaks.
//
// Two helpers cover the matrix:
//
//   * `make_point()` declares a `Point` literal split across four
//     lines (open brace alone, each field on its own line, close
//     brace alone).  Recorder must surface `p` as
//     `ValueRecord::Struct { fields: [Int(3), Int(4)] }`.
//   * `make_triple()` declares a tuple literal split across four
//     lines (open paren alone, each element on its own line, close
//     paren alone).  Recorder must surface `t` as
//     `ValueRecord::Tuple { elements: [Int(11), Int(22), Int(33)] }`.

struct Point {
    x: int;
    y: int;
}

fun make_point(): int {
    var p: Point = Point {
        x: 3,
        y: 4
    };
    var sq: int = p.x * p.x + p.y * p.y;
    return sq;
}

fun make_triple(): int {
    var t: (int, int, int) = (
        11,
        22,
        33
    );
    var first: int = t.0;
    var second: int = t.1;
    var third: int = t.2;
    var total: int = first + second + third;
    return total;
}

fun compute(): int {
    var pt: int = make_point();
    var tr: int = make_triple();
    var grand: int = pt + tr;
    return grand;
}

fun main(): int {
    return compute();
}
