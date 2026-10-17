// P17-T4c 解密 helper（由工具以 ELECTRON_RUN_AS_NODE 模式在 WorkBuddy.exe
// 内运行）。安全边界：atRestSecretKey 只在本进程内存出现；解密出的 token
// 仅经 stdout 管道返回给工具进程——不落盘、不进日志。
// 兼容双形态：旧版明文字符串字段直接透传；5.6.2+ 的 $wbEncrypted 信封
// 取钥解密（AAD 构造已经真机 GCM 校验验证，见 spec 8.4）。
const fs = require('fs');
const crypto = require('crypto');

function lp(s) {
  const b = Buffer.from(s, 'utf8');
  const head = Buffer.alloc(4);
  head.writeUInt32BE(b.length, 0);
  return Buffer.concat([head, b]);
}

function fieldAad(keyId, suite) {
  return Buffer.concat([
    Buffer.from('WB-AAD\0', 'latin1'),
    Buffer.from([0x01]),
    lp('WBEV1'),
    lp('sym-v1'),
    (() => { const b = Buffer.alloc(4); b.writeUInt32BE(suite, 0); return b; })(),
    lp(keyId),
    Buffer.from([0x02]), // framing=field
    Buffer.from([0x00]), // sequence 缺省
    Buffer.from([0x00]), // final=false
  ]);
}

// token 字段双形态：明文字符串（旧版）或 {$wbEncrypted, envelope}（5.6.2+）。
function resolveTokenField(field) {
  if (typeof field === 'string') {
    return { ok: true, plain: field, source: 'plain' };
  }
  if (field && field.$wbEncrypted === 1 && typeof field.envelope === 'string') {
    return { ok: false, source: 'envelope', envelope: field.envelope };
  }
  return { ok: false, source: 'unknown' };
}

function decryptEnvelope(envelopeB64, key, keyId) {
  const envelope = JSON.parse(Buffer.from(envelopeB64, 'base64').toString('utf8'));
  if (envelope.keyId !== keyId) {
    throw new Error(`keyId mismatch: envelope=${envelope.keyId} derived=${keyId}`);
  }
  const nonce = Buffer.from(envelope.nonce, 'base64');
  const tag = Buffer.from(envelope.authTag, 'base64');
  const ct = Buffer.from(envelope.ciphertext, 'base64');
  const decipher = crypto.createDecipheriv('aes-256-gcm', key, nonce, { authTagLength: 16 });
  decipher.setAAD(fieldAad(envelope.keyId, envelope.suite));
  decipher.setAuthTag(tag);
  return Buffer.concat([decipher.update(ct), decipher.final()]).toString('utf8');
}

const infoPath = process.argv[2];
if (!infoPath) {
  process.stdout.write(JSON.stringify({ ok: false, error: 'missing info path argument' }));
  process.exit(1);
}
const info = JSON.parse(fs.readFileSync(infoPath, 'utf8'));
const access = resolveTokenField(info.auth && info.auth.accessToken);
const refresh = resolveTokenField(info.auth && info.auth.refreshToken);

const needsKey = [access, refresh].some((r) => r.source === 'envelope');
let key = null;
let keyId = null;
if (needsKey) {
  const storage = process._linkedBinding('electron_browser_workbuddy_storage');
  const payload = JSON.parse(storage.loggerGet());
  key = crypto.createHash('sha256').update(payload.atRestSecretKey, 'utf8').digest();
  keyId = crypto.createHash('sha256').update(key).digest('hex').slice(0, 16);
}

function materialize(result) {
  if (result.ok) {
    return result.plain;
  }
  if (result.source === 'envelope') {
    return decryptEnvelope(result.envelope, key, keyId);
  }
  throw new Error('token field is neither plain string nor $wbEncrypted envelope');
}

try {
  process.stdout.write(JSON.stringify({
    ok: true,
    access_token: materialize(access),
    refresh_token: materialize(refresh),
    access_source: access.source,
    refresh_source: refresh.source,
  }));
} catch (error) {
  process.stdout.write(JSON.stringify({ ok: false, error: String(error && error.message || error) }));
  process.exit(1);
}
