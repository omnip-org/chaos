export interface AnalyticsProviderOptions {
  metaPixel?: { pixelId: string };
  ga4?: { measurementId: string };
}

export type AnalyticsErrorHandler = (
  error: unknown,
  event: { eventName: string; eventId?: string | undefined },
) => void;

type GtagDataLayerEntry = unknown[] | IArguments;
type FbqFunction = ((...args: unknown[]) => void) & {
  callMethod?: (...args: unknown[]) => void;
  queue?: unknown[][];
  loaded?: boolean;
  version?: string;
};

type AnalyticsWindow = Window &
  typeof globalThis & {
    dataLayer?: GtagDataLayerEntry[];
    gtag?: (...args: unknown[]) => void;
    fbq?: FbqFunction;
    _fbq?: FbqFunction;
  };

/** Loads and calls the configured browser analytics providers. */
export class AnalyticsDestinations {
  private readonly windowRef: AnalyticsWindow;
  private metaStarted = false;
  private ga4Started = false;
  private externalIdHash: string | null = null;
  private metaCustomerData: Record<string, string> = {};

  constructor(
    windowRef: Window & typeof globalThis,
    private readonly documentRef: Document,
    private readonly options: AnalyticsProviderOptions | undefined,
    private readonly onError: AnalyticsErrorHandler | undefined,
  ) {
    this.windowRef = windowRef as AnalyticsWindow;
    validateProviderOptions(options);
    if (options?.ga4) this.startGa4();
    if (options?.metaPixel) this.startMeta();
  }

  pixel(
    eventName: string,
    eventId: string,
    parameters: Record<string, unknown>,
  ): boolean {
    if (!this.metaStarted || !this.windowRef.fbq) return false;
    try {
      this.windowRef.fbq("track", eventName, parameters, { eventID: eventId });
      return true;
    } catch (error) {
      this.reportError(error, eventName, eventId);
      return false;
    }
  }

  get hasPixel(): boolean {
    return this.metaStarted;
  }

  get hasGa4(): boolean {
    return this.ga4Started;
  }

  setMetaCustomerData(values: Record<string, string>): void {
    this.metaCustomerData = values;
    this.updateMetaMatching();
  }

  setExternalId(hash: string | null): void {
    this.externalIdHash = hash;
    this.updateMetaMatching();
  }

  ga4(eventName: string, parameters: Record<string, unknown>): boolean {
    if (!this.ga4Started || !this.windowRef.gtag) return false;
    try {
      this.windowRef.gtag("event", eventName, parameters);
      return true;
    } catch (error) {
      this.reportError(
        error,
        eventName,
        typeof parameters.event_id === "string"
          ? parameters.event_id
          : undefined,
      );
      return false;
    }
  }

  private updateMetaMatching(): void {
    if (!this.metaStarted || !this.options?.metaPixel) return;
    try {
      this.windowRef.fbq?.("init", this.options.metaPixel.pixelId, {
        ...(this.externalIdHash ? { external_id: this.externalIdHash } : {}),
        ...this.metaCustomerData,
      });
    } catch (error) {
      this.reportError(error, "AdvancedMatching", undefined);
    }
  }

  private reportError(
    error: unknown,
    eventName: string,
    eventId: string | undefined,
  ): void {
    try {
      this.onError?.(error, { eventName, eventId });
    } catch {
      // Analytics failures never propagate into storefront operations.
    }
  }

  private startMeta(): void {
    if (this.metaStarted || !this.options?.metaPixel) return;
    this.metaStarted = true;
    if (!this.windowRef.fbq) {
      const fbq: FbqFunction = (...args: unknown[]) => {
        if (fbq.callMethod) fbq.callMethod(...args);
        else fbq.queue?.push(args);
      };
      fbq.queue = [];
      fbq.loaded = true;
      fbq.version = "2.0";
      this.windowRef.fbq = fbq;
      this.windowRef._fbq = fbq;
      loadProviderScript(
        this.documentRef,
        "chaos-meta-pixel",
        "https://connect.facebook.net/en_US/fbevents.js",
      );
    }
    this.windowRef.fbq("init", this.options.metaPixel.pixelId);
  }

  private startGa4(): void {
    if (this.ga4Started || !this.options?.ga4) return;
    this.ga4Started = true;
    const dataLayer = (this.windowRef.dataLayer ??= []);
    this.windowRef.gtag ??= function gtag() {
      dataLayer.push(arguments);
    };
    this.windowRef.gtag("js", new Date());
    this.windowRef.gtag("config", this.options.ga4.measurementId);
    loadProviderScript(
      this.documentRef,
      "chaos-google-tag",
      `https://www.googletagmanager.com/gtag/js?id=${encodeURIComponent(
        this.options.ga4.measurementId,
      )}`,
    );
  }
}

export function normalizeMetaText(
  value: string | undefined,
): string | undefined {
  if (!value) return undefined;
  const normalized = value.trim().toLowerCase().replace(/\s+/g, "");
  return normalized || undefined;
}

function validateProviderOptions(
  options: AnalyticsProviderOptions | undefined,
): void {
  if (options?.metaPixel && !/^[0-9]{5,32}$/.test(options.metaPixel.pixelId)) {
    throw new TypeError("providers.metaPixel.pixelId must contain 5-32 digits");
  }
  if (options?.ga4 && !/^G-[A-Z0-9]{4,20}$/.test(options.ga4.measurementId)) {
    throw new TypeError(
      "providers.ga4.measurementId must be a GA4 measurement ID",
    );
  }
}

function loadProviderScript(
  documentRef: Document,
  id: string,
  source: string,
): void {
  if (
    documentRef.getElementById?.(id) ||
    !documentRef.createElement ||
    !documentRef.head
  ) {
    return;
  }
  const script = documentRef.createElement("script");
  script.id = id;
  script.async = true;
  script.src = source;
  documentRef.head.appendChild(script);
}
