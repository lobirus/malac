package io.malac.core.state

import io.malac.core.protocol.AgentStatus
import io.malac.core.protocol.AgentView
import io.malac.core.protocol.PendingApproval
import io.malac.core.protocol.ServerMessage

/** What the last background check saw: requests already notified and agents that were running. */
data class Ledger(val requests: Set<String> = emptySet(), val running: Set<String> = emptySet())

data class CheckResult(
    /** Requests that were not notified yet. */
    val approvals: List<PendingApproval>,
    /** Top-level agents that were running at the last check and are not now. */
    val finished: List<AgentView>,
    /** Requests notified earlier that no longer wait: answered elsewhere, or gone with their agent. */
    val resolved: Set<String>,
    val ledger: Ledger,
)

/**
 * Compares a snapshot fetched in the background with the previous check.
 * The snapshot covers pending requests of every workspace but agents of
 * the daemon's last used workspace only.
 */
fun check(snapshot: ServerMessage.Snapshot, ledger: Ledger): CheckResult {
    val agents = snapshot.workspace?.tabs.orEmpty().flatMap { it.snapshots }.filter { it.parent == null }
    val running = agents.filter { it.live && it.status == AgentStatus.Running }.map { it.id }.toSet()
    val finished = agents.filter { it.id in ledger.running && it.id !in running && it.status != AgentStatus.Idle }
    val approvals = snapshot.pending.filter { it.id !in ledger.requests }
    // Only open requests are remembered, so the ledger stays small.
    val open = snapshot.pending.map { it.id }.toSet()
    return CheckResult(approvals, finished, ledger.requests - open, Ledger(requests = open, running = running))
}
