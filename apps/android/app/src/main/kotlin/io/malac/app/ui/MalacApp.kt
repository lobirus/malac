package io.malac.app.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.List
import androidx.compose.material.icons.filled.Home
import androidx.compose.material.icons.filled.Notifications
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation.NavDestination.Companion.hasRoute
import androidx.navigation.NavGraph.Companion.findStartDestination
import androidx.navigation.NavHostController
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.currentBackStackEntryAsState
import androidx.navigation.compose.rememberNavController
import androidx.navigation.toRoute
import io.malac.app.AppGraph
import io.malac.app.R
import io.malac.app.connection.ActiveDaemon
import io.malac.app.data.SavedDaemon
import io.malac.app.ui.agent.AgentScreen
import io.malac.app.ui.approvals.ApprovalsScreen
import io.malac.app.ui.components.CrashReport
import io.malac.app.ui.components.ImageViewerHost
import io.malac.app.ui.components.ImageViewerState
import io.malac.app.ui.components.LocalImageViewer
import io.malac.app.ui.daemons.AddDaemonScreen
import io.malac.app.ui.daemons.DaemonsScreen
import io.malac.app.ui.jobs.JobLogScreen
import io.malac.app.ui.jobs.JobsScreen
import io.malac.app.ui.talk.TalkScreen
import io.malac.app.ui.workspaces.TabScreen
import io.malac.app.ui.workspaces.WorkspaceScreen
import io.malac.app.ui.workspaces.WorkspacesScreen
import io.malac.core.net.DaemonConnection
import io.malac.core.state.DaemonState
import kotlinx.coroutines.flow.Flow
import kotlinx.serialization.Serializable

@Serializable data object WorkspacesRoute
@Serializable data class WorkspaceRoute(val id: String)
@Serializable data class TabRoute(val id: String)
@Serializable data class AgentRoute(val id: String? = null, val tab: String? = null)
@Serializable data object ApprovalsRoute
@Serializable data object JobsRoute
@Serializable data class JobRoute(val id: String)
@Serializable data object DaemonsRoute
@Serializable data object AddDaemonRoute
@Serializable data class TalkRoute(val tab: String? = null, val agent: String? = null)

/** A notification asked to show an agent of a daemon. */
data class OpenAgent(val daemon: String, val agent: String)

@Composable
fun MalacApp(graph: AppGraph, opens: Flow<OpenAgent>) {
    CrashReport(graph.crashes)
    val daemons by graph.daemons.daemons.collectAsStateWithLifecycle<List<SavedDaemon>?>(initialValue = null)
    val active by graph.connections.active.collectAsStateWithLifecycle()
    val saved = daemons ?: return
    val current = active
    if (saved.isEmpty() || current == null) {
        AddDaemonScreen(graph, onDone = {}, onBack = null)
        return
    }
    // Another daemon is another world: its navigation starts over.
    key(current.daemon.id) {
        Main(graph, current, opens)
    }
}

@Composable
private fun Main(graph: AppGraph, active: ActiveDaemon, opens: Flow<OpenAgent>) {
    val nav = rememberNavController()
    val snackbar = remember { SnackbarHostState() }
    val state by active.connection.state.collectAsStateWithLifecycle()
    val status by active.connection.status.collectAsStateWithLifecycle()
    val commands = rememberCommands(active) { graph.connections.notice(it) }
    val viewer = remember { ImageViewerState() }

    LaunchedEffect(graph) {
        graph.connections.notices.collect { snackbar.showSnackbar(it) }
    }
    // What malac says aloud, briefly, for when the sound is off.
    LaunchedEffect(graph) {
        graph.speech.lines.collect { snackbar.showSnackbar(it) }
    }
    // Images open over everything, edge to edge; what is under them is hidden from TalkBack meanwhile.
    CompositionLocalProvider(LocalImageViewer provides viewer) {
        Box(Modifier.fillMaxSize()) {
            Box(Modifier.fillMaxSize().then(if (viewer.image != null) Modifier.clearAndSetSemantics {} else Modifier)) {
                Screens(graph, active, opens, nav, snackbar, state, status, commands)
            }
            ImageViewerHost(viewer)
        }
    }
}

@Composable
private fun Screens(
    graph: AppGraph,
    active: ActiveDaemon,
    opens: Flow<OpenAgent>,
    nav: NavHostController,
    snackbar: SnackbarHostState,
    state: DaemonState,
    status: DaemonConnection.Status,
    commands: Commands,
) {
    LaunchedEffect(opens) {
        opens.collect { open ->
            if (open.daemon == active.daemon.id) nav.navigate(AgentRoute(open.agent)) else graph.daemons.select(open.daemon)
        }
    }

    val entry by nav.currentBackStackEntryAsState()
    val destination = entry?.destination
    val topLevel = listOf(WorkspacesRoute::class, ApprovalsRoute::class, JobsRoute::class)
    val showBar = destination == null || topLevel.any { destination.hasRoute(it) }

    Scaffold(
        snackbarHost = { SnackbarHost(snackbar) },
        bottomBar = {
            if (showBar) {
                Column {
                    HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
                    NavigationBar(containerColor = MaterialTheme.colorScheme.background, tonalElevation = 0.dp) {
                        TopLevelItem(nav, WorkspacesRoute, destination?.hasRoute(WorkspacesRoute::class) == true, Icons.Filled.Home, stringResource(R.string.nav_workspaces))
                        val waiting = state.pending.size
                        TopLevelItem(
                            nav,
                            ApprovalsRoute,
                            destination?.hasRoute(ApprovalsRoute::class) == true,
                            Icons.Filled.Notifications,
                            stringResource(R.string.nav_approvals),
                            badge = waiting.takeIf { it > 0 }?.let { pluralStringResource(R.plurals.approvals_waiting, it, it) },
                            count = waiting,
                        )
                        TopLevelItem(nav, JobsRoute, destination?.hasRoute(JobsRoute::class) == true, Icons.AutoMirrored.Filled.List, stringResource(R.string.nav_jobs))
                    }
                }
            }
        },
    ) { padding ->
        // Only the bottom is taken here; each screen's top bar pads itself
        // below the status bar and the camera cutout.
        val bottom = PaddingValues(bottom = padding.calculateBottomPadding())
        Column(Modifier.padding(bottom).consumeWindowInsets(bottom)) {
            NavHost(nav, startDestination = WorkspacesRoute) {
                composable<WorkspacesRoute> {
                    WorkspacesScreen(
                        active = active,
                        state = state,
                        status = status,
                        onOpenWorkspace = { nav.navigate(WorkspaceRoute(it)) },
                        onOpenDaemons = { nav.navigate(DaemonsRoute) },
                    )
                }
                composable<WorkspaceRoute> { backStack ->
                    WorkspaceScreen(
                        active = active,
                        state = state,
                        status = status,
                        workspace = backStack.toRoute<WorkspaceRoute>().id,
                        onOpenTab = { nav.navigate(TabRoute(it)) },
                        onBack = { nav.popBackStack() },
                    )
                }
                composable<TabRoute> { backStack ->
                    TabScreen(
                        state = state,
                        status = status,
                        tab = backStack.toRoute<TabRoute>().id,
                        onOpenAgent = { nav.navigate(AgentRoute(it)) },
                        onNewAgent = { nav.navigate(AgentRoute(tab = it)) },
                        onBack = { nav.popBackStack() },
                    )
                }
                composable<AgentRoute> { backStack ->
                    val route = backStack.toRoute<AgentRoute>()
                    AgentScreen(
                        active = active,
                        commands = commands,
                        agentId = route.id,
                        newInTab = route.tab,
                        status = status,
                        onOpenAgent = { nav.navigate(AgentRoute(it)) },
                        onStarted = { agent ->
                            nav.navigate(AgentRoute(agent)) { popUpTo<AgentRoute> { inclusive = true } }
                        },
                        onTalk = { tab, agent -> nav.navigate(TalkRoute(tab, agent)) },
                        onBack = { nav.popBackStack() },
                    )
                }
                composable<TalkRoute> { backStack ->
                    val route = backStack.toRoute<TalkRoute>()
                    TalkScreen(graph, active, route.tab, route.agent, onEnd = { nav.popBackStack() })
                }
                composable<ApprovalsRoute> {
                    ApprovalsScreen(state, status, commands, onOpenAgent = { nav.navigate(AgentRoute(it)) })
                }
                composable<JobsRoute> {
                    JobsScreen(state, status, active, onOpenJob = { nav.navigate(JobRoute(it)) })
                }
                composable<JobRoute> { backStack ->
                    JobLogScreen(active, state, commands, backStack.toRoute<JobRoute>().id, onBack = { nav.popBackStack() })
                }
                composable<DaemonsRoute> {
                    DaemonsScreen(graph, active, state, commands, onAdd = { nav.navigate(AddDaemonRoute) }, onBack = { nav.popBackStack() })
                }
                composable<AddDaemonRoute> {
                    AddDaemonScreen(graph, onDone = { nav.popBackStack(WorkspacesRoute, inclusive = false) }, onBack = { nav.popBackStack() })
                }
            }
        }
    }
}

@Composable
private fun RowScope.TopLevelItem(
    nav: NavHostController,
    route: Any,
    selected: Boolean,
    icon: ImageVector,
    label: String,
    badge: String? = null,
    count: Int = 0,
) {
    NavigationBarItem(
        selected = selected,
        onClick = {
            nav.navigate(route) {
                popUpTo(nav.graph.findStartDestination().id) { saveState = true }
                launchSingleTop = true
                restoreState = true
            }
        },
        icon = {
            BadgedBox(badge = { if (count > 0) Badge { Text(count.toString()) } }) {
                Icon(icon, contentDescription = null)
            }
        },
        label = { Text(label) },
        modifier = if (badge != null) Modifier.semantics { contentDescription = "$label, $badge" } else Modifier,
        colors = NavigationBarItemDefaults.colors(indicatorColor = MaterialTheme.colorScheme.secondaryContainer),
    )
}
