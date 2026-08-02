import { VersioningType, type INestApplication } from '@nestjs/common';

export interface HttpSetupOptions {
  /** Prefixes every business route. Probes are excluded from it, and stay unversioned. */
  readonly globalPrefix: string;
}

/**
 * Where the API's routes are mounted.
 *
 * One function rather than two lines in `main.ts`, because tests never run
 * `main.ts` and so have to reproduce this themselves. With only the prefix to
 * copy that was a nuisance; with versioning beside it the copy drifts silently
 * — a spec that forgets `enableVersioning` still boots, still routes, and
 * asserts against paths the deployed service does not serve.
 */
export function httpSetup(app: INestApplication, options: HttpSetupOptions): void {
  app.setGlobalPrefix(options.globalPrefix, {
    // Probe paths are infrastructure. Pinning them to `/health/*` keeps
    // deployment manifests independent of how the API is versioned.
    exclude: ['health/live', 'health/ready'],
  });

  // **No `defaultVersion`, deliberately.** `RoutesResolver.getVersionMetadata`
  // falls back to it for *every* controller, and the global-prefix exclude list
  // strips the prefix but never a version segment — so setting one would move
  // the probes to `/v1/health/live` and take the compose healthcheck, the
  // Kubernetes manifests and the drain sequence with it. Versioned routes opt
  // in with `@Version`, and a route that forgets to is caught by the OpenAPI
  // spec's hard-coded path list.
  app.enableVersioning({ type: VersioningType.URI });
}
