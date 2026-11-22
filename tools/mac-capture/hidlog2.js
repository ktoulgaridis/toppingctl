const ts = () => (Date.now() / 1000).toFixed(6);
const hex = (p, n) => { try { return Array.from(new Uint8Array(p.readByteArray(n)), x => x.toString(16).padStart(2,'0')).join(' '); } catch (e) { return '?'; } };
const fcc = v => { const s = String.fromCharCode((v>>>24)&255,(v>>>16)&255,(v>>>8)&255,v&255); return /^[ -~]{4}$/.test(s) ? s : v.toString(16); };
const ca = Process.getModuleByName('CoreAudio').findExportByName('AudioObjectSetPropertyData');
if (ca) Interceptor.attach(ca, { onEnter(a) {
  const addr = a[1]; const n = a[4].toInt32();
  console.log(`${ts()} ca obj=${a[0].toInt32()} sel=${fcc(addr.readU32())} scope=${fcc(addr.add(4).readU32())} el=${addr.add(8).readU32()} data=${hex(a[5], Math.min(n,64))}`);
}});
const tl = Process.findModuleByName('libtlusbdfuapi.dylib');
for (const name of ['TLDFU_InterfaceVendorOutRequest', 'TLDFU_InterfaceVendorInRequest']) {
  const fn = tl && tl.findExportByName(name); if (!fn) continue;
  Interceptor.attach(fn, { onEnter(a) {
    const args = []; for (let i = 0; i < 8; i++) args.push(a[i].toString());
    let dumps = []; for (let i = 1; i < 8; i++) { const p = a[i]; if (p.compare(ptr('0x100000000')) > 0) dumps.push(`a${i}=[${hex(p, 32)}]`); }
    console.log(`${ts()} ${name.endsWith('Out') ? 'vout' : 'vin'} args=${args.join(',')} ${dumps.join(' ')}`);
  }});
}
const ik = Process.getModuleByName('IOKit').findExportByName('IOHIDDeviceGetReport');
if (ik) Interceptor.attach(ik, { onEnter(a) { console.log(`${ts()} getreport type=${a[1].toInt32()} id=${a[2].toInt32()}`); }});
console.log(`${ts()} hidlog2 loaded ca=${!!ca} tl=${!!tl}`);
