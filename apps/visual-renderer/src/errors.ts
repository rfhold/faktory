export class ServiceError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
  ) {
    super(code);
  }
}

export function errorMessage(code: string): { error: string } {
  return { error: code.slice(0, 80) };
}
