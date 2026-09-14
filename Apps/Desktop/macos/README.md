# OpenPocketCine Camera (macOS)

The camera extension that makes the desktop viewfinder appear as **OpenPocketCine** in
every app that opens a camera on macOS 13 and later, and the small host app that
installs it. The viewfinder (`Apps/Desktop`) writes into the extension's sink stream
through the Swift facade (`Sources/OpenPocketCineDesktopFacade/DesktopVirtualCameraABI.swift`).

```sh
brew install xcodegen
cd Apps/Desktop/macos
xcodegen generate
xcodebuild -project OpenPocketCineCamera.xcodeproj -scheme OpenPocketCineCamera -configuration Release
```

Copy the built `OpenPocketCine Camera.app` to `/Applications`, open it, press
**Install**, approve the extension in System Settings → Privacy & Security. Then in the
viewfinder's Output tab set **Virtual camera** to **Camera device**.

What it needs, and why:

- The extension carries the system-extension entitlement, so the team in `project.yml`
  must be in the Apple Developer Program and the app Developer ID signed. Nothing loads
  otherwise.
- While developing, `systemextensionsctl developer on` lets a build run from anywhere
  instead of `/Applications`.
- The mach service name in `CameraExtension/Info.plist` and the app group in both
  entitlements files share the team prefix; change the bundle id in one place and the
  other two follow.

Nothing here has been built or run on a Mac yet; see `docs/desktop-viewfinder.md`.
