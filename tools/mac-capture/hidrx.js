// logs tx (SetReport) and rx (input report callback) from inside the app
const ts = () => (Date.now() / 1000).toFixed(6);
const hx = (p, n) => Array.from(new Uint8Array(p.readByteArray(n)), x => x.toString(16).padStart(2, '0')).join(' ');
const iokit = Process.getModuleByName('IOKit');
Interceptor.attach(iokit.findExportByName('IOHIDDeviceSetReport'), { onEnter(a) {
  console.log(`${ts()} tx ${hx(a[3], a[4].toInt32())}`);
}});
const keep = [];
Interceptor.attach(iokit.findExportByName('IOHIDDeviceRegisterInputReportCallback'), { onEnter(a) {
  // (device, report*, reportLength, callback, context)
  const orig = a[3];
  if (orig.isNull()) return;
  const origFn = new NativeFunction(orig, 'void', ['pointer', 'int', 'pointer', 'int', 'uint32', 'pointer', 'long']);
  const wrap = new NativeCallback(function (ctx, result, sender, type, id, report, len) {
    try { console.log(`${ts()} rx ${hx(report, Number(len))}`); } catch (e) {}
    origFn(ctx, result, sender, type, id, report, len);
  }, 'void', ['pointer', 'int', 'pointer', 'int', 'uint32', 'pointer', 'long']);
  keep.push(wrap, origFn);
  a[3] = wrap;
  console.log(`${ts()} hooked input callback`);
}});
