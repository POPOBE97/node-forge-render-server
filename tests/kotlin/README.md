# OMotion Ground Truth

`OMotionGroundTruthExporter.kt` is the only writer for
`../fixtures/omotion_spring_ground_truth.json`. It runs against the frozen
`motion-core:0.1.0-alpha02-SNAPSHOT` AAR used by voice-interaction commit
`b3e4abb`.

The fixture contains direct OMotion Spring scenarios. Every frame records value, velocity, target,
driver, and exact running/completed state.

The Rust test suite only reads the checked-in JSON. Regeneration is an explicit
maintenance operation; it is not allowed to resolve or execute Kotlin during
CI.
