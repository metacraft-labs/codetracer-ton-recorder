// Exercises Tolk's `type` aliases and the `as T` cast operator.
//
// Tolk supports `type Coins = int;` style aliases pervasively in
// real-world contracts — `Coins`, `Workchain`, `MessageFlags`, etc.
// are all bare aliases for `int` (and occasionally for other numeric
// shapes).  The cast operator `<expr> as <T>` is the surface used to
// move a value across the alias boundary (e.g. `msg_value as Coins`)
// and Tolk treats it as a no-op at the value level — the underlying
// `Int` ValueRecord must round-trip unchanged.
//
// Pre-this-fixture:
//   * `type Coins = int;` was silently swallowed by the recorder's
//     line-oriented parser (the only `type X = A | B | C;` shape it
//     recognised was the variant-constructor union scanner).
//   * `var balance: Coins = msg_value as Coins;` would either fail to
//     parse (the `as Coins` suffix is not part of the int-only TVM
//     expression grammar) or parse the `: Coins` annotation against
//     an unregistered type id.
//
// Spec-compliant recording surfaces:
//   * the alias preserves the underlying `Int` ValueRecord (kind ==
//     "Int", `i` == the source numeric value);
//   * the `as T` cast parses as a no-op — the surrounding step count
//     stays exactly equal to the var-binding count, no spurious
//     extra step gets emitted for the cast itself.

type Coins = int;
type Workchain = int;

fun pay(msg_value: int): int {
    var balance: Coins = msg_value as Coins;
    var fee: Coins = 5 as Coins;
    var net: Coins = balance - fee;
    return net;
}

fun route(): int {
    var wc: Workchain = 0 as Workchain;
    var paid: int = pay(100);
    var combined: int = paid + wc;
    return combined;
}

fun compute(): int {
    return route();
}

fun main(): int {
    return compute();
}
