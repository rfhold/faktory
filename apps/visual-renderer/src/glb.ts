import { ServiceError } from "./errors.js";

const JSON_CHUNK = 0x4e4f534a;
const BIN_CHUNK = 0x004e4942;

export function validateGlb(bytes: Buffer): void {
  if (bytes.length < 20 || bytes.toString("ascii", 0, 4) !== "glTF") {
    throw new ServiceError(422, "malformed_glb");
  }
  if (bytes.readUInt32LE(4) !== 2 || bytes.readUInt32LE(8) !== bytes.length) {
    throw new ServiceError(422, "malformed_glb");
  }
  let offset = 12;
  let chunks = 0;
  while (offset < bytes.length) {
    if (offset + 8 > bytes.length) throw new ServiceError(422, "malformed_glb");
    const length = bytes.readUInt32LE(offset);
    const type = bytes.readUInt32LE(offset + 4);
    offset += 8;
    if (length === 0 || length % 4 !== 0 || offset + length > bytes.length) {
      throw new ServiceError(422, "malformed_glb");
    }
    if ((chunks === 0 && type !== JSON_CHUNK) || (chunks === 1 && type !== BIN_CHUNK) || chunks > 1) {
      throw new ServiceError(422, "malformed_glb");
    }
    if (chunks === 0) {
      try {
        const document: unknown = JSON.parse(bytes.toString("utf8", offset, offset + length).trimEnd());
        if (typeof document !== "object" || document === null || Array.isArray(document)) throw new Error();
      } catch {
        throw new ServiceError(422, "malformed_glb");
      }
    }
    offset += length;
    chunks += 1;
  }
  if (offset !== bytes.length || chunks < 1) throw new ServiceError(422, "malformed_glb");
}
