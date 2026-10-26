import MalacKit
import SwiftUI

/// A job's log tail, refreshed every two seconds while it runs, and Stop.
struct JobLogView: View {
    let session: DaemonSession
    let jobId: String
    @State private var output: JobOutput?
    @State private var error: String?
    @State private var confirmStop = false

    private static let tail = 400

    var body: some View {
        let state = session.state
        let job = state.jobs[jobId]
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    if let job {
                        VStack(alignment: .leading, spacing: 6) {
                            StatusLabel(phase: JobStatusText.phase(job))
                            Text(job.command).font(.callout.monospaced()).textSelection(.enabled)
                            facts(job, home: state.daemon?.userHome)
                        }
                        if let agent = job.agent, state.agents[agent] != nil {
                            NavigationLink(value: Route.agent(agent)) {
                                Label("Started by \(state.agents[agent]?.displayName ?? "an agent")", systemImage: "person")
                                    .font(.footnote)
                            }
                            .tint(Palette.green)
                        }
                    }
                    if let error {
                        Label(error, systemImage: "exclamationmark.triangle").font(.footnote).foregroundStyle(Palette.red)
                    }
                    ScrollView(.horizontal) {
                        Text(output.map { $0.text.isEmpty ? "(no output yet)" : $0.text } ?? "Loading…")
                            .font(.system(.caption, design: .monospaced))
                            .textSelection(.enabled)
                            .fixedSize(horizontal: true, vertical: false)
                            .padding(10)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .card(Palette.surface)
                    Color.clear.frame(height: 1).id("end")
                }
                .padding(16)
            }
            .background(Palette.page)
            .onChange(of: output?.text) { proxy.scrollTo("end", anchor: .bottom) }
        }
        .navigationTitle("Job \(jobId)")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            if job?.isRunning == true {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Stop", role: .destructive) { confirmStop = true }.tint(Palette.red)
                }
            }
        }
        .confirmationDialog("Stop this job?", isPresented: $confirmStop, titleVisibility: .visible) {
            Button("Stop job", role: .destructive) { Task { await session.kill(job: jobId) } }
        } message: {
            Text("It gets SIGTERM, then SIGKILL after 3 seconds.")
        }
        .task(id: session.isOnline) { await poll() }
    }

    @ViewBuilder
    private func facts(_ job: JobView, home: String?) -> some View {
        Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 4) {
            GridRow {
                Text("Folder").foregroundStyle(Palette.text3)
                Text(Format.displayPath(job.cwd, home: home)).font(.footnote.monospaced())
            }
            if let started = job.started {
                GridRow {
                    Text("Ran").foregroundStyle(Palette.text3)
                    TimelineView(.periodic(from: .now, by: 1)) { context in
                        Text("\(JobStatusText.runtime(job, now: context.date) ?? ""), since \(started.formatted(date: .omitted, time: .shortened))")
                    }
                }
            }
            if let output, output.truncated {
                GridRow {
                    Text("Shown").foregroundStyle(Palette.text3)
                    Text("Last \(Self.tail) lines of \(ByteCountFormatter.string(fromByteCount: Int64(output.size), countStyle: .file))")
                }
            }
        }
        .font(.footnote)
    }

    private func poll() async {
        guard session.isOnline else { return }
        while !Task.isCancelled {
            do {
                output = try await session.jobOutput(jobId, tail: Self.tail)
                error = nil
            } catch {
                self.error = error.localizedDescription
            }
            guard session.state.jobs[jobId]?.isRunning == true else { return }
            try? await Task.sleep(for: .seconds(2))
        }
    }
}
