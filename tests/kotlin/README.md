# OMotion Ground Truth

`OMotionGroundTruthExporter.kt` is the only writer for
`../fixtures/omotion_spring_ground_truth.json`. It runs against the frozen
`motion-core:0.1.0-alpha02-SNAPSHOT` AAR used by voice-interaction commit
`b3e4abb`.

The fixture contains both direct OMotion Spring scenarios and Kotlin reference
composites for stopped-Hold/Timeline/Tween and running-Spring/Timeline/Tween.
Every frame records value, velocity, driver, Timeline progress, Tween weight,
and exact running/completed state.

The Rust test suite only reads the checked-in JSON. Regeneration is an explicit
maintenance operation; it is not allowed to resolve or execute Kotlin during
CI.
