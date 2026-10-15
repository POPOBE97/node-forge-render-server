# Native Screen Test players

Build/install, USB protocol, chart format, HDR semantics and validation status are documented in
[Screen Test](../../docs/screen-test.md). Android uses Kotlin/OpenGL ES; iOS uses UIKit/Metal. Both
display host-generated FP16 pixels at native resolution and expose only a loopback USB endpoint.

No camera controls, reference database, chart layout or color fitting run on the phone. Keep the
player foreground during acquisition. Generated Xcode projects, build products and local SDK paths
are intentionally ignored. Set your own signing team through `DEVELOPMENT_TEAM` when building iOS.
