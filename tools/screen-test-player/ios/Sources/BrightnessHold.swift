import Foundation

enum DisplayThermalState: String {
    case nominal = "Nominal", fair = "Fair", serious = "Serious", critical = "Critical", unknown = "Unknown"
    var permitsCapture: Bool { self == .nominal || self == .fair }
}

protocol BrightnessHardware: AnyObject {
    var brightness: Double { get set }
    var idleTimerDisabled: Bool { get set }
    var thermalState: DisplayThermalState { get }
    var lowPowerMode: Bool { get }
    var uptime: TimeInterval { get }
}

/// Owns the brightness setting only while a test chart is foreground. All calls
/// run on the main thread. Revision detects interruptions even after recovery.
final class BrightnessHold {
    private let hardware: BrightnessHardware
    private var active = false
    private var previousBrightness: Double?
    private var previousIdleTimer: Bool?
    private var stableSince: TimeInterval?
    private var lastWrite = -Double.infinity
    private var mismatched = false
    private var lastThermal: DisplayThermalState?
    private var lastLowPower: Bool?
    private(set) var target: Double?
    private(set) var revision: UInt64 = 0
    static let tolerance = 0.005
    static let settlingSeconds = 1.0

    init(hardware: BrightnessHardware) { self.hardware = hardware }

    func setActive(_ value: Bool) {
        guard active != value else { return }
        active = value
        if target != nil { revision += 1 }
        stableSince = nil
        if active { acquire(); poll() } else { restore() }
    }

    func begin(_ brightness: Double) {
        precondition(brightness.isFinite && (0.0...1.0).contains(brightness))
        target = brightness
        revision += 1
        stableSince = nil
        mismatched = false
        lastThermal = hardware.thermalState
        lastLowPower = hardware.lowPowerMode
        acquire()
        // This is an intentional target change, not an unexpected interruption.
        if active && hardware.thermalState.permitsCapture && !hardware.lowPowerMode {
            hardware.brightness = brightness
            lastWrite = hardware.uptime
        }
        poll()
    }

    func end() {
        if target != nil { revision += 1 }
        target = nil
        stableSince = nil
        restore()
    }

    private func acquire() {
        guard active, target != nil, previousBrightness == nil else { return }
        previousBrightness = hardware.brightness
        previousIdleTimer = hardware.idleTimerDisabled
        hardware.idleTimerDisabled = true
    }

    private func restore() {
        // Clear ownership before writing: UIKit can post brightness notifications.
        let brightness = previousBrightness
        let idleTimer = previousIdleTimer
        previousBrightness = nil
        previousIdleTimer = nil
        if let brightness { hardware.brightness = brightness }
        if let idleTimer { hardware.idleTimerDisabled = idleTimer }
    }

    func poll() {
        guard active, let target else { return }
        let thermal = hardware.thermalState
        let lowPower = hardware.lowPowerMode
        if lastThermal != thermal || lastLowPower != lowPower {
            revision += 1
            stableSince = nil
            lastThermal = thermal
            lastLowPower = lowPower
        }
        if !hardware.idleTimerDisabled {
            revision += 1
            stableSince = nil
            hardware.idleTimerDisabled = true
        }
        // Do not fight thermal protection. The host must wait for safe conditions.
        guard thermal.permitsCapture, !lowPower else { stableSince = nil; return }
        if abs(hardware.brightness - target) > Self.tolerance {
            if !mismatched { revision += 1 }
            mismatched = true
            stableSince = nil
            if hardware.uptime - lastWrite >= 0.25 {
                hardware.brightness = target
                lastWrite = hardware.uptime
            }
        } else {
            mismatched = false
            if stableSince == nil { stableSince = hardware.uptime }
        }
    }

    var settled: Bool {
        guard active, let target, let stableSince else { return false }
        return hardware.idleTimerDisabled && hardware.thermalState.permitsCapture
            && !hardware.lowPowerMode && abs(hardware.brightness - target) <= Self.tolerance
            && hardware.uptime - stableSince >= Self.settlingSeconds
    }

    var issue: String? {
        guard target != nil else { return nil }
        if !active { return "Keep Screen Test in the foreground." }
        if !hardware.thermalState.permitsCapture {
            return "iPhone thermal state is \(hardware.thermalState.rawValue). Let it cool before capture; iOS may dim the display."
        }
        if hardware.lowPowerMode { return "Turn off Low Power Mode before capture." }
        if !settled { return "Waiting for the requested brightness to remain stable for one second." }
        return nil
    }

    func status() -> [String: Any] {
        poll()
        var result: [String: Any] = [
            "holding": active && target != nil && previousBrightness != nil,
            "idle_timer_disabled": hardware.idleTimerDisabled,
            "settled": settled, "revision": revision,
            "thermal_state": hardware.thermalState.rawValue,
            "low_power_mode": hardware.lowPowerMode,
        ]
        if let target { result["requested_brightness"] = target }
        if let issue { result["issue"] = issue }
        return result
    }
}
