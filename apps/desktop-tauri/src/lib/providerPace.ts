/**
 * Whether a provider snapshot may show pace advice. The backend marks
 * non-authoritative fetches with `paceAuthoritative: false`; device-local
 * OpenCode Go quota estimates do not establish account-wide pace either.
 */
export function providerAllowsPace(
  providerId: string,
  sourceLabel: string | null | undefined,
  paceAuthoritative?: boolean,
): boolean {
  if (paceAuthoritative === false) return false;
  return !(
    providerId === "opencodego" &&
    sourceLabel?.trim().toLowerCase() === "local estimate"
  );
}
