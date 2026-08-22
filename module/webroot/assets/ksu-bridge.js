let seq = 0;
export function exec(command, options = {}) {
  return new Promise((resolve, reject) => {
    const callback = `xiao_exec_${Date.now()}_${seq++}`;
    window[callback] = (errno, stdout, stderr) => {
      delete window[callback];
      resolve({ errno, stdout, stderr });
    };
    try {
      // KernelSU's current WebUI manager exposes the same native `ksu.exec(cmd, options,
      // callback)` bridge used by the official `kernelsu` npm package. xiao keeps this
      // tiny vendored adapter so the installable module has no runtime CDN/npm dependency.
      // Commands are fixed binaries plus base64url payloads; form data is never shell syntax.
      if (!globalThis.ksu || typeof globalThis.ksu.exec !== 'function') {
        throw new Error('KernelSU WebUI exec API is unavailable');
      }
      globalThis.ksu.exec(command, JSON.stringify(options || {}), callback);
    } catch (e) {
      delete window[callback];
      reject(e);
    }
  });
}
