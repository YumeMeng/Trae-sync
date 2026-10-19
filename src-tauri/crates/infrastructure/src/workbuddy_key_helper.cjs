// P18-B2 切号实验：取钥 helper（由工具以 ELECTRON_RUN_AS_NODE 模式在
// WorkBuddy.exe 内运行）。安全边界：原始 atRestSecretKey 只在本进程内存
// 出现、**永不离开子进程**——本 helper 只输出 sha256 派生后的 32 字节密钥
// （hex）与其 keyId（加密信封构造的唯一所需材料），比解密 helper 的暴露面
// 更小。与 workbuddy_decrypt_helper.cjs 的派生链逐字节一致：
//   key  = sha256(atRestSecretKey utf8)
//   keyId = sha256(key).hex 前 16 位
const crypto = require('crypto');
try {
  const storage = process._linkedBinding('electron_browser_workbuddy_storage');
  const payload = JSON.parse(storage.loggerGet());
  const key = crypto.createHash('sha256').update(payload.atRestSecretKey, 'utf8').digest();
  const keyId = crypto.createHash('sha256').update(key).digest('hex').slice(0, 16);
  process.stdout.write(JSON.stringify({ ok: true, key_hex: key.toString('hex'), key_id: keyId }));
} catch (error) {
  process.stdout.write(JSON.stringify({ ok: false, error: String(error && error.message || error) }));
  process.exit(1);
}
