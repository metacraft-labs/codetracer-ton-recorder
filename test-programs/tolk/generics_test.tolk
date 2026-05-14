// Exercises Tolk's generic functions and generic structs.  Tolk
// supports parametric polymorphism via Rust-style angle brackets:
// `fun max<T>(a: T, b: T): T` declares a function generic over `T`,
// and `max<int>(3, 5)` instantiates it with `T = int` at the call
// site.  Real-world Tolk contracts use this pattern for collection
// helpers (`Vec<T>::push`, `Map<K, V>::get`) and for numeric-domain
// helpers that work uniformly over `int` / `coin` / `Coins` aliases.
//
// Pre-this-fixture the recorder's hand-rolled parser would treat
// `max<T>` as the literal name `max<T>` (the `<` survived the
// `find('(')` cut), and the call site `max<int>(3, 5)` similarly
// became a lookup for the function name `max<int>` — neither
// resolved against the registered `max<T>` declaration.
//
// Spec-compliant recording surfaces:
//   * the generic helper registers under the bare name `max` (the
//     `<T>` is stripped at registration so monomorphised call sites
//     find it),
//   * `Box<T>` registers as `Box` for the same reason,
//   * each monomorphised instantiation surfaces with matching scalar
//     ValueRecord shapes on both formals (i.e. `max<int>` returns an
//     Int — NOT erased to a single T placeholder).

struct Box<T> {
    value: int;
}

fun max<T>(a: T, b: T): T {
    var winner: int = a;
    if (b > a) {
        winner = b;
    }
    return winner;
}

fun take_box(box: Box<int>): int {
    return box.value;
}

fun compute(): int {
    var x: int = max<int>(3, 5);
    var y: int = max<int>(50, 50);
    var b: Box<int> = Box<int> { value: 7 };
    var z: int = take_box(b);
    var total: int = x + y + z;
    return total;
}

fun main(): int {
    return compute();
}
