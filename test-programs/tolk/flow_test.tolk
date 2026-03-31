fun compute(): int {
    var a: int = 10;
    var b: int = 32;
    var sum_val: int = a + b;
    var doubled: int = sum_val * 2;
    var final_result: int = doubled + a;
    return final_result;
}

fun main(): int {
    return compute();
}
