const immutableImage = /^\S+@sha256:[0-9a-f]{64}$/;
const dnsName = /^[a-z0-9](?:[a-z0-9.-]{0,251}[a-z0-9])?$/;
const segment = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/;

export function requireImmutableImage(value: string): string {
  if (!immutableImage.test(value)) throw new Error("image must use an immutable sha256 digest");
  return value;
}

export function validateHttpsOrigin(value: string, name: string): string {
  const url = new URL(value);
  if (url.protocol !== "https:" || url.username || url.password || url.pathname !== "/" || url.search || url.hash) {
    throw new Error(`${name} must be a credential-free HTTPS origin`);
  }
  return url.origin;
}

export function cimdTrustedPrivateOrigins(value: unknown): string[] {
  if (value === undefined) return [];
  const invalid = () => new Error("CIMD requires at most 16 exact HTTPS origins within the runtime byte limits");
  if (!Array.isArray(value) || value.length > 16) throw invalid();
  const origins = value.map((entry: unknown) => {
    if (typeof entry !== "string") throw invalid();
    const origin = entry.trim();
    if (Buffer.byteLength(origin) > 512 || !/^https:\/\/[^/?#\\@*,]+\/?$/.test(origin)) throw invalid();
    let url: URL;
    try { url = new URL(origin); } catch { throw invalid(); }
    if (url.hostname.includes("*") || Buffer.byteLength(url.href) > 512) throw invalid();
    return url.origin;
  });
  if (Buffer.byteLength(origins.join(",")) > 4096 ||
      origins.reduce((bytes, origin) => bytes + Buffer.byteLength(`${origin}/`), 0) > 4096) throw invalid();
  return [...new Set(origins)];
}

export function validateHostname(value: string): string {
  if (!dnsName.test(value) || value.includes("..")) throw new Error("hostname is invalid");
  return value;
}

export function validateSegment(value: string, name: string): string {
  if (!segment.test(value)) throw new Error(`${name} is invalid`);
  return value;
}

export function companionConfiguration(hostname: string | undefined, image: string | undefined, appHostname: string) {
  if (hostname === undefined) return undefined;
  const host = validateHostname(hostname);
  host.split(".").forEach((label) => validateSegment(label, "companionHostname label"));
  if (host === appHostname || host.endsWith(`.${appHostname}`) || appHostname.endsWith(`.${host}`)) {
    throw new Error("companionHostname must be separate from the app hostname and its cookie domain");
  }
  if (!image) throw new Error("companionImage is required when companionHostname is set");
  return { hostname: host, origin: `https://${host}`, image: requireImmutableImage(image) };
}
