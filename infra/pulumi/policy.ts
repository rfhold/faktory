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

export function validateHostname(value: string): string {
  if (!dnsName.test(value) || value.includes("..")) throw new Error("hostname is invalid");
  return value;
}

export function validateSegment(value: string, name: string): string {
  if (!segment.test(value)) throw new Error(`${name} is invalid`);
  return value;
}
