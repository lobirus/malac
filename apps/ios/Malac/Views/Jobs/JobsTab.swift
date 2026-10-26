import MalacKit
import SwiftUI

/// Background jobs the agents started, newest first.
struct JobsTab: View {
    var body: some View {
        NavigationStack {
            SessionContent { session in
                JobList(session: session)
                    .navigationDestination(for: Route.self) { route in
                        switch route {
                        case .job(let id): JobLogView(session: session, jobId: id)
                        case .agent(let id): AgentScreen(session: session, agentId: id, tabId: nil)
                        default: EmptyView()
                        }
                    }
            }
            .navigationTitle("Jobs")
        }
    }
}

private struct JobList: View {
    let session: DaemonSession

    var body: some View {
        let state = session.state
        let jobs = state.jobs.values.sorted { ($0.started ?? .distantPast) > ($1.started ?? .distantPast) }
        List {
            ForEach(jobs) { job in
                NavigationLink(value: Route.job(job.id)) {
                    JobRow(job: job, home: state.daemon?.userHome)
                }
            }
            .listRowBackground(Palette.surface)
        }
        .malacList()
        .overlay {
            if state.isLoaded, jobs.isEmpty {
                ContentUnavailableView("No jobs", systemImage: "terminal",
                                       description: Text("Commands agents run in the background appear here."))
            }
        }
    }
}

private struct JobRow: View {
    let job: JobView
    let home: String?

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            StatusDot(phase: JobStatusText.phase(job)).padding(.top, 6)
            VStack(alignment: .leading, spacing: 3) {
                Text(job.command).font(.callout.monospaced()).lineLimit(2)
                TimelineView(.periodic(from: .now, by: 1)) { context in
                    Text([JobStatusText.text(job), JobStatusText.runtime(job, now: context.date), Format.displayPath(job.cwd, home: home)]
                        .compactMap { $0 }.joined(separator: " · "))
                        .font(.footnote.monospacedDigit())
                        .foregroundStyle(Palette.text2)
                        .lineLimit(1)
                        .truncationMode(.head)
                }
            }
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
    }
}

enum JobStatusText {
    static func phase(_ job: JobView) -> Phase {
        switch job.status {
        case .running: .running
        case .killed: .stopped
        case .exited: (job.exitCode ?? 0) == 0 ? .done : .failed
        case .unknown: .idle
        }
    }

    static func text(_ job: JobView) -> String {
        switch job.status {
        case .running: "Running"
        case .killed: "Killed"
        case .exited: job.exitCode.map { "Exited \($0)" } ?? "Exited"
        case .unknown: "Unknown"
        }
    }

    static func runtime(_ job: JobView, now: Date) -> String? {
        guard let started = job.started else { return nil }
        return Format.duration(from: started, to: job.finished ?? now)
    }
}
