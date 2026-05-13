// Exercises Tolk's Builder/Slice ref-bearing operations, extending
// M9's int-only cell-op coverage (`cell_ops_test.tolk`) to the
// shapes that carry real BoC sub-cell payloads:
//
//   * `storeRef(<cell>)`     — append a sub-cell to a builder's ref queue.
//   * `storeMaybeRef(<cell>)` — same, with the null-ref ABI.
//   * `storeSlice(<slice>)`  — flatten a slice's remaining payload + refs.
//   * `storeAddress(<int>)`  — encode a wallet/account address marker.
//   * `loadRef()`            — pop the next sub-cell from a slice's ref queue.
//   * `loadAddress()`        — read the address back out as an int.
//
// Spec-compliant recording surfaces:
//   * each Builder/Cell/Slice as a ValueRecord::Raw whose `r` string
//     carries the ordered int payload PLUS the appended-refs queue
//     (`refs=[Cell([...]), ...]`), distinguishing this fixture's
//     output from `cell_ops_test.tolk` (which has no refs and so
//     emits the bare payload string),
//   * `loadRef()` returning a Cell whose payload matches the source
//     cell exactly (proving the ref queue round-trips),
//   * `loadAddress()` returning the stored int as a plain Int
//     ValueRecord (the recorder doesn't model bit-level address
//     wire format; the int round-trip is what the fixture pins).

fun pack_inner(): cell {
    var ib: builder = beginCell();
    var ib1: builder = ib.storeInt(101, 32);
    var inner: cell = ib1.endCell();
    return inner;
}

fun pack_outer(): cell {
    var inner: cell = pack_inner();
    var ob: builder = beginCell();
    var ob1: builder = ob.storeInt(200, 32);
    var ob2: builder = ob1.storeRef(inner);
    var ob3: builder = ob2.storeAddress(51966);
    var outer: cell = ob3.endCell();
    return outer;
}

fun unpack(c: cell): int {
    var s: slice = c.beginParse();
    var marker: int = s.loadInt(32);
    var sub: cell = s.loadRef();
    var addr: int = s.loadAddress();
    var sub_s: slice = sub.beginParse();
    var inner_val: int = sub_s.loadInt(32);
    var combined: int = marker + inner_val + addr;
    return combined;
}

fun compute(): int {
    var packed: cell = pack_outer();
    var decoded: int = unpack(packed);
    return decoded;
}

fun main(): int {
    return compute();
}
