// Exercises Tolk's structured value kinds: tuples and structs.
//
// Tolk tuples are heterogeneous fixed-size sequences (`(int, int)`),
// and modern Tolk supports `struct` declarations with named fields.
// Spec-compliant recording would surface:
//   * tuple literals as `ValueRecord::Tuple { elements: [...] }`,
//   * struct literals as `ValueRecord::Struct { fields: [...] }`,
//   * field accesses as scalar reads of the appropriate element.
//
// The current Tolk parser is line-oriented and treats tuple / struct
// literals as opaque expression text, so the only values that surface
// today are the integer let-bindings inside the zero-arg helper
// (`scalar_only`).  See RECORDER BUG notes in the matching test for
// the spec-compliant expectation.

struct Point {
    x: int;
    y: int;
}

fun sum_pair(): int {
    var pair: (int, int) = (10, 20);
    var first: int = pair.0;
    var second: int = pair.1;
    var pair_sum: int = first + second;
    return pair_sum;
}

fun point_distance_sq(): int {
    var p: Point = Point { x: 3, y: 4 };
    var sq: int = p.x * p.x + p.y * p.y;
    return sq;
}

fun scalar_only(): int {
    var head_val: int = 1;
    var len: int = 4;
    var total: int = head_val + len;
    return total;
}

fun compute(): int {
    var pair_total: int = sum_pair();
    var point_total: int = point_distance_sq();
    var scalar_total: int = scalar_only();
    var grand_total: int = pair_total + point_total + scalar_total;
    return grand_total;
}

fun main(): int {
    return compute();
}
