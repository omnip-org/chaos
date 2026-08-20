/// <reference types="astro/client" />

interface ImportMetaEnv {
  readonly PUBLIC_CHAOS_PUBLISHABLE_KEY: string;
  readonly PUBLIC_CHAOS_STORE_API_BASE_URL?: string;
  readonly PUBLIC_CHAOS_ANALYTICS_PRIVACY_MODE?: "opt_in" | "opt_out";
  readonly PUBLIC_META_PIXEL_ID?: string;
  readonly PUBLIC_GA4_MEASUREMENT_ID?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
