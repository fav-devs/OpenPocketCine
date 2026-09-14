// The host: installs the camera extension and says how it went. That is all it does;
// once the extension is in, the viewfinder finds the camera on its own.

import SwiftUI
import SystemExtensions

@main
struct CameraHostApp: App {
    @StateObject private var manager = ExtensionManager()

    var body: some Scene {
        WindowGroup("OpenPocketCine Camera") {
            VStack(alignment: .leading, spacing: 16) {
                Text("OpenPocketCine Camera").font(.title2).bold()
                Text(
                    "Installs the camera extension that shows the viewfinder as a camera to every other app. Keep this app in /Applications; the extension lives inside it."
                )
                .fixedSize(horizontal: false, vertical: true)
                Text(manager.status).font(.callout).foregroundStyle(.secondary)
                HStack {
                    Button("Install") { manager.install() }
                        .keyboardShortcut(.defaultAction)
                    Button("Remove") { manager.remove() }
                }
            }
            .padding(24)
            .frame(width: 440)
        }
        .windowResizability(.contentSize)
    }
}

final class ExtensionManager: NSObject, ObservableObject, OSSystemExtensionRequestDelegate {
    @Published var status = "Not installed, or not checked yet."

    private var identifier: String {
        (Bundle.main.bundleIdentifier ?? "com.opencapture.openpocketcine.camera") + ".extension"
    }

    func install() {
        let request = OSSystemExtensionRequest.activationRequest(
            forExtensionWithIdentifier: identifier, queue: .main)
        request.delegate = self
        OSSystemExtensionManager.shared.submitRequest(request)
        status = "Installing…"
    }

    func remove() {
        let request = OSSystemExtensionRequest.deactivationRequest(
            forExtensionWithIdentifier: identifier, queue: .main)
        request.delegate = self
        OSSystemExtensionManager.shared.submitRequest(request)
        status = "Removing…"
    }

    func request(
        _ request: OSSystemExtensionRequest, actionForReplacingExtension existing: OSSystemExtensionProperties,
        withExtension ext: OSSystemExtensionProperties
    ) -> OSSystemExtensionRequest.ReplacementAction {
        .replace
    }

    func requestNeedsUserApproval(_ request: OSSystemExtensionRequest) {
        status = "Approve it in System Settings → Privacy & Security, then come back."
    }

    func request(
        _ request: OSSystemExtensionRequest, didFinishWithResult result: OSSystemExtensionRequest.Result
    ) {
        switch result {
        case .completed:
            status = "Done. \"OpenPocketCine\" is a camera now; turn it on in the viewfinder's System tab."
        case .willCompleteAfterReboot:
            status = "Done after a restart."
        @unknown default:
            status = "Finished."
        }
    }

    func request(_ request: OSSystemExtensionRequest, didFailWithError error: Error) {
        status = "Failed: \(error.localizedDescription)"
    }
}
