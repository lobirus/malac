package io.malac.core.transcript

/** One line of a line diff. */
data class DiffLine(val op: Op, val text: String) {
    enum class Op { Same, Added, Removed }
}

private const val MAX_CELLS = 400_000

/**
 * Line diff of [before] against [after]: the common prefix and suffix are
 * trimmed before the quadratic longest-common-subsequence part, which is
 * skipped for very large inputs.
 */
fun diffLines(before: String, after: String): List<DiffLine> {
    val a = if (before.isEmpty()) emptyList() else before.split('\n')
    val b = if (after.isEmpty()) emptyList() else after.split('\n')
    var start = 0
    while (start < a.size && start < b.size && a[start] == b[start]) start++
    var endA = a.size
    var endB = b.size
    while (endA > start && endB > start && a[endA - 1] == b[endB - 1]) {
        endA--
        endB--
    }
    val midA = a.subList(start, endA)
    val midB = b.subList(start, endB)
    val ops = a.subList(0, start).mapTo(mutableListOf()) { DiffLine(DiffLine.Op.Same, it) }
    if (midA.size.toLong() * midB.size > MAX_CELLS) {
        midA.mapTo(ops) { DiffLine(DiffLine.Op.Removed, it) }
        midB.mapTo(ops) { DiffLine(DiffLine.Op.Added, it) }
    } else {
        val n = midA.size
        val m = midB.size
        val table = Array(n + 1) { IntArray(m + 1) }
        for (i in n - 1 downTo 0) {
            for (j in m - 1 downTo 0) {
                table[i][j] = if (midA[i] == midB[j]) table[i + 1][j + 1] + 1 else maxOf(table[i + 1][j], table[i][j + 1])
            }
        }
        var i = 0
        var j = 0
        while (i < n || j < m) {
            when {
                i < n && j < m && midA[i] == midB[j] -> {
                    ops += DiffLine(DiffLine.Op.Same, midA[i++])
                    j++
                }
                i < n && (j == m || table[i + 1][j] >= table[i][j + 1]) -> ops += DiffLine(DiffLine.Op.Removed, midA[i++])
                else -> ops += DiffLine(DiffLine.Op.Added, midB[j++])
            }
        }
    }
    a.subList(endA, a.size).mapTo(ops) { DiffLine(DiffLine.Op.Same, it) }
    return ops
}
