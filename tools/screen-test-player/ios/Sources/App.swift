import UIKit
import Metal
import QuartzCore
import Network
import CryptoKit

@main final class AppDelegate: UIResponder, UIApplicationDelegate {
    var window: UIWindow?
    var server: PlayerServer?
    func application(_ application: UIApplication, didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?) -> Bool {
        let controller = PlayerController()
        let window = UIWindow(frame: UIScreen.main.bounds)
        window.rootViewController = controller
        self.window = window
        window.makeKeyAndVisible()
        server = PlayerServer(controller: controller)
        server?.start()
        return true
    }
    func applicationDidBecomeActive(_ application: UIApplication) {
        (window?.rootViewController as? PlayerController)?.brightnessHold.setActive(true)
    }
    func applicationWillResignActive(_ application: UIApplication) {
        (window?.rootViewController as? PlayerController)?.brightnessHold.setActive(false)
    }
    func applicationWillTerminate(_ application: UIApplication) {
        (window?.rootViewController as? PlayerController)?.brightnessHold.end()
    }
}

final class ScreenBrightnessHardware: BrightnessHardware {
    var brightness: Double {
        get { Double(UIScreen.main.brightness) }
        set { UIScreen.main.brightness = CGFloat(newValue) }
    }
    var idleTimerDisabled: Bool {
        get { UIApplication.shared.isIdleTimerDisabled }
        set { UIApplication.shared.isIdleTimerDisabled = newValue }
    }
    var uptime: TimeInterval { ProcessInfo.processInfo.systemUptime }
    var lowPowerMode: Bool { ProcessInfo.processInfo.isLowPowerModeEnabled }
    var thermalState: DisplayThermalState {
        switch ProcessInfo.processInfo.thermalState {
        case .nominal: return .nominal
        case .fair: return .fair
        case .serious: return .serious
        case .critical: return .critical
        @unknown default: return .unknown
        }
    }
}

final class MetalView: UIView {
    override class var layerClass: AnyClass { CAMetalLayer.self }
    var metalLayer: CAMetalLayer { layer as! CAMetalLayer }
}

final class PlayerController: UIViewController {
    let brightnessHold = BrightnessHold(hardware: ScreenBrightnessHardware())
    var brightnessTimer: Timer?
    var brightnessObservers: [NSObjectProtocol] = []
    let metalView = MetalView()
    let device = MTLCreateSystemDefaultDevice()!
    lazy var queue = device.makeCommandQueue()!
    var pipeline: MTLRenderPipelineState?
    var texture: MTLTexture?
    var generation = ""
    var requestedGeneration = ""
    var mode = "Srgb"
    var lastError: String?
    var displayLink: CADisplayLink?
    let label = UILabel()
    override var prefersStatusBarHidden: Bool { true }
    override var prefersHomeIndicatorAutoHidden: Bool { true }
    override func loadView() { view = metalView }
    override func viewDidLoad() {
        super.viewDidLoad()
        let layer = metalView.metalLayer
        layer.device = device
        layer.pixelFormat = .rgba16Float
        layer.framebufferOnly = true
        layer.colorspace = CGColorSpace(name: CGColorSpace.extendedLinearSRGB)
        label.text = "Node Forge Screen Test\nUSB connected · waiting for chart\nDisable True Tone, Night Shift and Auto-Brightness"
        label.textColor = .lightGray; label.numberOfLines = 0; label.textAlignment = .center
        view.addSubview(label)
        brightnessHold.setActive(UIApplication.shared.applicationState == .active)
        brightnessTimer = Timer(timeInterval: 0.25, repeats: true) { [weak self] _ in
            self?.brightnessHold.poll()
        }
        RunLoop.main.add(brightnessTimer!, forMode: .common)
        for name in [UIScreen.brightnessDidChangeNotification,
                     ProcessInfo.thermalStateDidChangeNotification,
                     Notification.Name.NSProcessInfoPowerStateDidChange] {
            brightnessObservers.append(NotificationCenter.default.addObserver(
                forName: name, object: nil, queue: .main
            ) { [weak self] _ in
                // Defer our own brightness notification to avoid recursive writes.
                DispatchQueue.main.async { self?.brightnessHold.poll() }
            })
        }
        let source = """
        #include <metal_stdlib>
        using namespace metal;
        struct V { float4 position [[position]]; float2 uv; };
        vertex V vtx(uint id [[vertex_id]]) {
            float2 p[3]={float2(-1,-1),float2(3,-1),float2(-1,3)};
            V o; o.position=float4(p[id],0,1); o.uv=float2((p[id].x+1)/2,1-(p[id].y+1)/2); return o;
        }
        fragment float4 frag(V v [[stage_in]], texture2d<half> chart [[texture(0)]]) {
            constexpr sampler s(coord::normalized, address::clamp_to_edge, filter::nearest);
            return float4(chart.sample(s,v.uv));
        }
        """
        do {
            let library = try device.makeLibrary(source: source, options: nil)
            let descriptor = MTLRenderPipelineDescriptor()
            descriptor.vertexFunction = library.makeFunction(name: "vtx")
            descriptor.fragmentFunction = library.makeFunction(name: "frag")
            descriptor.colorAttachments[0].pixelFormat = .rgba16Float
            pipeline = try device.makeRenderPipelineState(descriptor: descriptor)
        } catch { lastError = error.localizedDescription }
        displayLink = CADisplayLink(target: self, selector: #selector(displayTick))
        displayLink?.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 60, preferred: 60)
        displayLink?.add(to: .main, forMode: .common)
    }
    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        let size = UIScreen.main.nativeBounds.size
        metalView.metalLayer.drawableSize = size
        label.frame = view.bounds.insetBy(dx: 30, dy: 100)
    }
    func status() -> [String: Any] {
        let brightnessStatus = brightnessHold.status()
        let screen = UIScreen.main
        let scale = screen.nativeScale
        let inset = view.safeAreaInsets
        let native = screen.nativeBounds.size
        let drawable = metalView.metalLayer.drawableSize
        var result: [String: Any] = ["id":"usb", "platform":"Ios", "name":UIDevice.current.name,
            "size":[Int(native.width),Int(native.height)],"drawable":[Int(drawable.width),Int(drawable.height)],
            "insets":[Int(ceil(inset.left*scale)),Int(ceil(inset.bottom*scale)),Int(ceil(inset.right*scale)),Int(ceil(inset.top*scale))],
            "modes":["Srgb","DisplayP3"] + (screen.potentialEDRHeadroom > 1 ? ["LinearHdr"] : []),
            "brightness":Double(screen.brightness),"headroom":Double(screen.currentEDRHeadroom),
            "active":UIApplication.shared.applicationState == .active,"generation":generation,
            "brightness_guard":brightnessStatus]
        if let error = lastError { result["error"] = error }
        return result
    }
    func present(_ header: [String:Any], bytes: Data) throws {
        guard let manifest = header["manifest"] as? [String:Any], let size = manifest["size"] as? [Int], size.count == 2,
              let hash = manifest["sha256"] as? String, let generation = manifest["generation"] as? String,
              let mode = manifest["mode"] as? String, let brightness = header["brightness"] as? Double,
              size[0] > 0, size[1] > 0, bytes.count == size[0]*size[1]*8 else { throw PlayerError.invalid("Invalid chart payload") }
        let digest = SHA256.hash(data: bytes).map { String(format:"%02x",$0) }.joined()
        guard digest == hash else { throw PlayerError.invalid("Chart checksum mismatch") }
        guard size == [Int(UIScreen.main.nativeBounds.width),Int(UIScreen.main.nativeBounds.height)] else { throw PlayerError.invalid("Chart must match native screen pixels") }
        guard ["Srgb","DisplayP3","LinearHdr"].contains(mode), brightness >= 0.0, brightness <= 1 else { throw PlayerError.invalid("Invalid mode or brightness") }
        guard UIApplication.shared.applicationState == .active else { throw PlayerError.invalid("Keep Screen Test in the foreground") }
        self.generation = ""
        self.requestedGeneration = generation
        let layer = metalView.metalLayer
        layer.wantsExtendedDynamicRangeContent = mode == "LinearHdr"
        layer.colorspace = CGColorSpace(name: mode == "DisplayP3" ? CGColorSpace.extendedLinearDisplayP3 : CGColorSpace.extendedLinearSRGB)
        let descriptor = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba16Float, width: size[0], height: size[1], mipmapped: false)
        descriptor.storageMode = .shared; descriptor.usage = .shaderRead
        guard let texture = device.makeTexture(descriptor: descriptor) else { throw PlayerError.invalid("Cannot allocate chart texture") }
        bytes.withUnsafeBytes { pointer in texture.replace(region: MTLRegionMake2D(0,0,size[0],size[1]), mipmapLevel: 0, withBytes: pointer.baseAddress!, bytesPerRow: size[0]*8) }
        brightnessHold.begin(brightness)
        self.texture = texture; self.mode = mode; lastError = nil; label.isHidden = true
        draw(generationToConfirm: generation)
    }
    func releaseDisplay() {
        brightnessHold.end()
        generation = ""
        requestedGeneration = ""
        texture = nil
        label.isHidden = false
    }
    @objc func displayTick() { draw(generationToConfirm: nil) }
    func draw(generationToConfirm: String?) {
        guard let texture = texture, let pipeline = pipeline, let drawable = metalView.metalLayer.nextDrawable(), let buffer = queue.makeCommandBuffer() else { return }
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = drawable.texture
        pass.colorAttachments[0].loadAction = .dontCare; pass.colorAttachments[0].storeAction = .store
        guard let encoder = buffer.makeRenderCommandEncoder(descriptor: pass) else { return }
        encoder.setRenderPipelineState(pipeline); encoder.setFragmentTexture(texture,index:0)
        encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3); encoder.endEncoding()
        if let value = generationToConfirm {
            let confirm: () -> Void = { [weak self] in
                guard let self, self.requestedGeneration == value else { return }
                self.generation = value
            }
            #if targetEnvironment(simulator)
            buffer.addCompletedHandler { _ in DispatchQueue.main.async(execute: confirm) }
            #else
            drawable.addPresentedHandler { _ in DispatchQueue.main.async(execute: confirm) }
            #endif
        }
        buffer.present(drawable); buffer.commit()
    }
}
enum PlayerError: Error { case invalid(String) }

final class PlayerServer {
    weak var controller: PlayerController?
    var listener: NWListener?
    let queue = DispatchQueue(label:"com.nodeforge.screentest.usb")
    init(controller:PlayerController) { self.controller = controller }
    func start() {
        do {
            let parameters = NWParameters.tcp
            parameters.requiredLocalEndpoint = .hostPort(host: "127.0.0.1", port: 48731)
            listener = try NWListener(using: parameters)
            listener?.newConnectionHandler = { [weak self] connection in
                guard let self = self else { return }
                connection.start(queue: self.queue)
                self.read(connection,count:4) { data in
                    guard let data = data, data.count == 4 else { connection.cancel(); return }
                    let n = data.enumerated().reduce(0) { $0 | (Int($1.element) << (8*$1.offset)) }
                    guard n > 0, n <= 1_048_576 else { connection.cancel(); return }
                    self.read(connection,count:n) { data in
                        guard let data = data, let header = (try? JSONSerialization.jsonObject(with:data)) as? [String:Any], let command = header["command"] as? String else { self.reply(connection,["error":"Invalid request"]); return }
                        let length = header["payload_bytes"] as? Int ?? 0
                        guard length >= 0, length <= 160_000_000 else { self.reply(connection,["error":"Payload too large"]); return }
                        self.read(connection,count:length) { payload in
                            guard let payload = payload else { connection.cancel(); return }
                            DispatchQueue.main.async {
                                guard let controller = self.controller else { connection.cancel(); return }
                                do {
                                    if command == "present" { try controller.present(header,bytes:payload) }
                                    else if command == "release" { controller.releaseDisplay() }
                                    else if command != "status" { throw PlayerError.invalid("Unknown command") }
                                    self.reply(connection,controller.status())
                                } catch { self.reply(connection,["error":"\(error)"]) }
                            }
                        }
                    }
                }
            }
            listener?.start(queue:queue)
        } catch { controller?.lastError = "USB listener: \(error)" }
    }
    func read(_ connection:NWConnection,count:Int,completion:@escaping(Data?)->Void) {
        if count == 0 { completion(Data()); return }
        let data = NSMutableData()
        func next() {
            let remaining = count-data.length
            connection.receive(minimumIncompleteLength:1, maximumLength:min(remaining,65536)) { bytes, _, done, error in
                if let bytes = bytes { data.append(bytes) }
                if data.length == count { completion(data as Data) }
                else if done || error != nil { completion(nil) }
                else { next() }
            }
        }
        next()
    }
    func reply(_ connection:NWConnection,_ value:[String:Any]) {
        guard let json = try? JSONSerialization.data(withJSONObject:value) else { connection.cancel(); return }
        var length = UInt32(json.count).littleEndian
        var bytes = Data(bytes:&length,count:4); bytes.append(json)
        connection.send(content:bytes,completion:.contentProcessed { _ in connection.cancel() })
    }
}
