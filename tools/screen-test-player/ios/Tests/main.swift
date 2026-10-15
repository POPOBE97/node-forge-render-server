import Foundation

final class FakeScreen: BrightnessHardware {
    var brightness = 0.3
    var idleTimerDisabled = false
    var thermalState = DisplayThermalState.nominal
    var lowPowerMode = false
    var uptime = 0.0
}

let screen = FakeScreen()
let hold = BrightnessHold(hardware: screen)
hold.setActive(true)
assert(!screen.idleTimerDisabled, "Opening the app alone should not disable Auto-Lock")
hold.begin(0.7)
assert(screen.brightness == 0.7 && screen.idleTimerDisabled)
assert(!hold.settled)
screen.uptime = 1.1
hold.poll()
assert(hold.settled)
let beforeCapture = hold.revision

// The setting can recover between the pre-shutter and post-download status calls.
// The revision must still make that capture invalid.
screen.brightness = 0.4
hold.poll()
assert(screen.brightness == 0.7 && !hold.settled)
assert(hold.revision > beforeCapture)
screen.uptime = 1.4
hold.poll()
screen.uptime = 2.5
hold.poll()
assert(hold.settled && hold.revision > beforeCapture)
let steadyRevision = hold.revision
for _ in 0..<10 { hold.poll() }
assert(hold.revision == steadyRevision, "Our own notifications must not invalidate stable captures")

// Thermal dimming may not change UIScreen.brightness; gate on thermal state too.
screen.thermalState = .serious
hold.poll()
assert(!hold.settled && hold.issue!.contains("cool"))
assert(hold.revision > steadyRevision)
screen.brightness = 0.2
screen.uptime = 3
hold.poll()
assert(screen.brightness == 0.2, "Never fight thermal protection")
screen.thermalState = .nominal
hold.poll()
assert(screen.brightness == 0.7 && !hold.settled)
screen.uptime = 3.3
hold.poll()
screen.uptime = 4.4
hold.poll()
assert(hold.settled)
screen.lowPowerMode = true
hold.poll()
assert(!hold.settled && hold.issue!.contains("Low Power"))
screen.lowPowerMode = false
hold.poll()

let beforeBackground = hold.revision
hold.setActive(false)
assert(screen.brightness == 0.3 && !screen.idleTimerDisabled)
assert(!hold.settled && hold.revision > beforeBackground)
screen.brightness = 0.45 // User changes their setting outside the app.
screen.uptime = 5
hold.setActive(true)
assert(screen.brightness == 0.7 && screen.idleTimerDisabled)
hold.begin(0.8)
hold.begin(0.6)
hold.end()
assert(screen.brightness == 0.45 && !screen.idleTimerDisabled,
       "Changing chart brightness must not replace the saved user preference")
hold.end()
assert(screen.brightness == 0.45 && hold.target == nil)

hold.begin(0.0)
assert(screen.brightness == 0.0, "Zero is a real requested brightness, not 0.01")
screen.uptime += 1.1
hold.poll()
assert(hold.settled && hold.target == 0.0)
hold.end()
assert(screen.brightness == 0.45)

// Respect a pre-existing idle-timer owner when releasing this session.
screen.idleTimerDisabled = true
hold.begin(0.5)
hold.end()
assert(screen.idleTimerDisabled)
print("Brightness hold: settling, drift recovery, interruption history, thermal/power gating, lifecycle and restoration passed")
