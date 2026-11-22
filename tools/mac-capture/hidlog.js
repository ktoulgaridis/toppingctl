const iokit = Process.getModuleByName('IOKit');
for (const name of ['IOHIDDeviceSetReport', 'IOHIDDeviceSetReportWithCallback']) {
  const fn = iokit.findExportByName(name);
  if (!fn) continue;
  Interceptor.attach(fn, { onEnter(args) {          // (dev, type, id, report*, len, ...)
    const n = args[4].toInt32();
    const b = new Uint8Array(args[3].readByteArray(n));
    console.log(`${(Date.now() / 1000).toFixed(6)} tx ` +
      Array.from(b, x => x.toString(16).padStart(2, '0')).join(' '));
  }});
}
