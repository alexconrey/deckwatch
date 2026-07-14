import { apiFetch } from "./client";

export type Tier = "community" | "pro" | "enterprise";

export interface LicenseLimits {
  max_users: number | null;
  max_clusters: number | null;
}

export interface CustomerView {
  org: string;
  contact: string;
}

export interface FeatureCatalogEntry {
  feature: string;
  tier: Tier;
  label: string;
  description: string;
}

export interface LicenseStatus {
  tier: Tier;
  expires_at: string | null;
  in_grace: boolean;
  features: string[];
  feature_catalog: FeatureCatalogEntry[];
  limits: LicenseLimits;
  customer: CustomerView | null;
  license_id: string | null;
  seconds_until_expiry: number | null;
}

/**
 * The license endpoint is intentionally public (readable without a Pro/Enterprise
 * gate) so the Community UI can render "Upgrade to Pro" affordances even
 * without a license. Community fallback response has tier="community" and an
 * empty features array.
 */
export const licenseApi = {
  get: () => apiFetch<LicenseStatus>("/license"),
};
