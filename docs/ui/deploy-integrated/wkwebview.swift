import Cocoa
import WebKit

final class D: NSObject, WKNavigationDelegate {
    let w: WKWebView
    var out: String
    var fin: (() -> Void)?
    var started = false
    func webView(_ wv: WKWebView, didFinish n: WKNavigation!) { print("didFinish"); if !started { started = true; fin?() } }
    func webView(_ wv: WKWebView, didFail n: WKNavigation!, withError e: Error) { print("fail \(e)") }
    func webView(_ wv: WKWebView, didFailProvisionalNavigation n: WKNavigation!, withError e: Error) { print("failProv \(e)") }
    init(_ w: WKWebView, _ out: String) { self.w = w; self.out = out }
    func js(_ s: String) -> String {
        var r = ""; var done = false
        w.evaluateJavaScript(s) { v, e in r = e != nil ? "ERR \(e!)" : "\(v ?? "nil")"; done = true }
        while !done { RunLoop.current.run(mode: .default, before: Date(timeIntervalSinceNow: 0.05)) }
        return r
    }
    func wait(_ t: Double) { RunLoop.current.run(until: Date(timeIntervalSinceNow: t)) }
    func shot(_ n: String) {
        var done = false
        w.takeSnapshot(with: nil) { img, _ in
            if let img = img, let t = img.tiffRepresentation, let b = NSBitmapImageRep(data: t), let p = b.representation(using: .png, properties: [:]) { try? p.write(to: URL(fileURLWithPath: "\(self.out)/\(n).png")) }
            done = true
        }
        while !done { RunLoop.current.run(mode: .default, before: Date(timeIntervalSinceNow: 0.05)) }
    }
    func click(_ label: String) -> String {
        js("(() => { const norm = x => x.innerText.replace(/\\s+/g,' ').trim(); const b = [...document.querySelectorAll('button')].filter(x => !x.disabled).find(x => norm(x) === \(label.debugDescription)); if (!b) return 'missing'; b.click(); return 'ok'; })()")
    }
    func sel(_ label: String, _ v: String) -> String {
        js("(() => { const l = [...document.querySelectorAll('.dp-sim label')].find(x => x.innerText.trim().startsWith('\(label)')); const s = l.querySelector('select'); Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype,'value').set.call(s,'\(v)'); s.dispatchEvent(new Event('change',{bubbles:true})); return s.value })()")
    }
    func run() {
        func rep(_ n: String, _ v: String) { print("\(n): \(v)") }
        print("start"); wait(2.5); _ = js("window.__errs=[]; window.addEventListener('error',e=>window.__errs.push(String(e.message))); 1")
        if CommandLine.arguments.count > 3 {
            _ = js("window.__errs=[]; window.addEventListener('error',e=>window.__errs.push(String(e.message))); window.addEventListener('unhandledrejection',e=>window.__errs.push('rej '+String(e.reason))); 1")
            func nav2(_ l: String) { _ = js("(() => { [...document.querySelectorAll('.shell-nav__item')].find(b => b.getAttribute('aria-label')==='\(l)').click(); return 'ok' })()"); wait(0.8); print("after \(l): " + js("document.querySelector('.shell-header__title')?.innerText.split('\\n')[0] ?? ''") + " errs=" + js("JSON.stringify(window.__errs)")) }
            _ = js("(() => { [...document.querySelectorAll('.shell-nav__item')].find(b => b.getAttribute('aria-label')==='Deployments').click(); return 1 })()"); wait(0.5)
            _ = sel("Runtime", "stopped"); wait(0.5)
            nav2("Containers"); nav2("Images")
            _ = sel("Runtime", "connected"); wait(0.5)
            nav2("Deployments"); _ = sel("Runtime", "stopped"); wait(0.5); nav2("Containers")
            exit(0)
        }
        if false {
            func nav(_ l: String) { _ = js("(() => { [...document.querySelectorAll('.shell-nav__item')].find(b => b.getAttribute('aria-label')==='\(l)').click(); return 'ok' })()"); wait(0.8); print("after \(l): " + js("document.querySelector('.shell-header__title')?.innerText ?? ''")) }
            print("initial: " + js("document.querySelector('.shell-header__title')?.innerText ?? ''"))
            nav("Containers"); nav("Deployments"); nav("Containers"); nav("Images")
            exit(0)
        }

        rep("ua", js("navigator.userAgent"))
        rep("nav has Deployments", js("!![...document.querySelectorAll('.shell-nav__item')].find(b => b.getAttribute('aria-label')==='Deployments')"))
        rep("open Deployments", js("(() => { [...document.querySelectorAll('.shell-nav__item')].find(b => b.getAttribute('aria-label')==='Deployments').click(); return 'ok' })()"))
        wait(1); shot("wk-01-home")
        rep("simulated label", js("/Simulated data/.test(document.querySelector('[data-testid=deployments-view]').innerText)"))
        rep("open api", js("(() => { [...document.querySelectorAll('.dp-card')].find(x => /api/.test(x.innerText)).click(); return 'ok' })()"))
        wait(0.6); shot("wk-02-app")
        rep("scroll container", js("(() => { const els=[...document.querySelectorAll('*')].filter(e=>{const s=getComputedStyle(e);return /(auto|scroll)/.test(s.overflowY)&&e.scrollHeight>e.clientHeight+5}); els.forEach(e=>e.scrollTop=e.scrollHeight); return els.map(e=>e.className.toString().slice(0,30)+':'+e.scrollTop).join(' | ') })()"))
        wait(0.3); shot("wk-03-scrolled")
        rep("deploy dialog", click("Deploy")); wait(0.5)
        rep("dialog shown", js("!!document.querySelector('[role=dialog]')")); shot("wk-04-dialog")
        rep("cancel", click("Cancel")); wait(0.3)
        rep("dev disabled", js("(() => { [...document.querySelectorAll('.dp-seg button')].find(b=>b.innerText.trim()==='Development').click(); return 'ok' })()")); wait(0.4)
        rep("dev actions all disabled", js("[...document.querySelectorAll('.dp-actions button')].every(b=>b.disabled)")); shot("wk-05-development")
        rep("prod", js("(() => { [...document.querySelectorAll('.dp-seg button')].find(b=>b.innerText.trim()==='Production').click(); return 'ok' })()"))
        rep("tab Variables", js("(() => { [...document.querySelectorAll('.dp-tabs button')].find(b=>/^Variables/.test(b.innerText.trim())).click(); return 'ok' })()"))
        rep("keychain locked", sel("Keychain", "locked")); wait(0.4)
        rep("how to unlock", click("How to unlock")); wait(0.4)
        rep("help dialog", js("/does not touch your real Keychain/.test(document.body.innerText)")); shot("wk-06-unlock-help")
        rep("close", click("Close")); _ = sel("Keychain", "unlocked")
        rep("dialog still?", js("!!document.querySelector('[role=dialog]')"))
        _ = js("(() => { [...document.querySelectorAll('.shell-nav__item')].find(b => b.getAttribute('aria-label')==='Containers').click(); return 'ok' })()"); wait(0.8)
        rep("mid-nav title", js("document.querySelector('.shell-header__title')?.innerText ?? ''"))
        _ = js("(() => { [...document.querySelectorAll('.shell-nav__item')].find(b => b.getAttribute('aria-label')==='Deployments').click(); return 'ok' })()"); wait(0.8)
        rep("runtime stopped", sel("Runtime", "stopped")); wait(1.2)
        rep("not live shown", js("/Harpoon is stopped/.test(document.querySelector('[data-testid=deployments-view]').innerText) && !/RUNNING/.test(document.querySelector('[data-testid=deployments-view]').innerText) && /UNKNOWN/.test(document.querySelector('[data-testid=deployments-view]').innerText)")); shot("wk-07-disconnected")
        rep("no app card claims RUNNING", js("![...document.querySelectorAll('.dp-card')].some(c=>/RUNNING/.test(c.innerText))"))
        _ = js("(() => { [...document.querySelectorAll('.shell-nav__item')].find(b => b.getAttribute('aria-label')==='Containers').click(); return 'ok' })()"); wait(0.8)
        rep("back to Containers", js("document.querySelector('.shell-header__title .text-title')?.innerText ?? document.querySelector('.shell-header__title')?.innerText"))
        exit(0)
    }
}

setvbuf(stdout, nil, _IOLBF, 0)
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let out = CommandLine.arguments[2]
let wv = WKWebView(frame: NSRect(x: 0, y: 0, width: 1280, height: 900))
let win = NSWindow(contentRect: wv.frame, styleMask: [.titled], backing: .buffered, defer: false)
win.contentView = wv
win.orderBack(nil)
let d = D(wv, out)
wv.navigationDelegate = d
wv.load(URLRequest(url: URL(string: CommandLine.arguments[1])!))
d.fin = { Timer.scheduledTimer(withTimeInterval: 0.5, repeats: false) { _ in d.run() } }
app.run()
