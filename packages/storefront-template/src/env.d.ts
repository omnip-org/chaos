/// <reference types="astro/client" />

interface ImportMetaEnv {
  readonly PUBLIC_CHAOS_PUBLISHABLE_KEY: string;
  readonly PUBLIC_CHAOS_STORE_API_BASE_URL?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
