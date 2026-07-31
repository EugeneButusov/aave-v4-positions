import type { INestApplication } from '@nestjs/common';
import { DocumentBuilder, SwaggerModule, type OpenAPIObject } from '@nestjs/swagger';

/**
 * Version of the *API contract*, not of the package. They are legitimately
 * different things: a dependency bump changes the package version and must not
 * imply anything about the shape of the responses.
 */
export const API_CONTRACT_VERSION = '0.1.0';

const DESCRIPTION = [
  'Indexed Aave v4 user positions, enriched and served for querying.',
  '',
  'Positions in v4 have no token representation — no `aToken`, no `Transfer` events.',
  'They exist only as structs in Spoke storage, so everything served here is folded',
  'from the Spoke and Hub event streams rather than read back from chain state.',
  'Protocol details and the derivations behind each field are in',
  '`docs/aave-v4-protocol-analysis.md`.',
  '',
  '**Integer amounts are strings.** Share balances exceed the 53-bit mantissa of a',
  'JSON number, and the failure mode of getting this wrong is a few wei of drift',
  'that reads as a rounding bug rather than a parse error.',
].join('\n');

/**
 * Built separately from {@link setupOpenApi} so tests can assert the document
 * without standing up the UI. The contract test uses this exact function, which
 * is what makes it a real check rather than a parallel definition.
 */
export function buildOpenApiDocument(app: INestApplication): OpenAPIObject {
  const config = new DocumentBuilder()
    .setTitle('Aave v4 Positions API')
    .setDescription(DESCRIPTION)
    .setVersion(API_CONTRACT_VERSION)
    .addTag('health', 'Kubernetes probes. Outside the API prefix, and unversioned.')
    .build();

  return SwaggerModule.createDocument(app, config);
}

/**
 * Serves Swagger UI and the raw document.
 *
 * The path is left outside the global API prefix, alongside `/health` — both are
 * operational surface rather than part of the versioned API.
 */
export function setupOpenApi(app: INestApplication, options: { path: string }): void {
  SwaggerModule.setup(options.path, app, () => buildOpenApiDocument(app), {
    jsonDocumentUrl: `${options.path}/openapi.json`,
    yamlDocumentUrl: `${options.path}/openapi.yaml`,
    customSiteTitle: 'Aave v4 Positions API',
    swaggerOptions: {
      // Endpoints sorted by path rather than declaration order, so the page does
      // not reshuffle when a controller is added.
      operationsSorter: 'alpha',
      tagsSorter: 'alpha',
      displayRequestDuration: true,
    },
  });
}
