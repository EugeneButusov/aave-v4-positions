import type { INestApplication } from '@nestjs/common';
import type { OpenAPIObject } from '@nestjs/swagger';
import { Test } from '@nestjs/testing';
import request from 'supertest';
import type { App } from 'supertest/types';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { AppModule } from '../src/app.module';
import { httpSetup } from '../src/http.setup';
import { API_CONTRACT_VERSION, buildOpenApiDocument, setupOpenApi } from '../src/openapi/openapi';

const DOCS_PATH = 'docs';

/**
 * Only `OpenAPIObject` is exported from the package root; the nested node types
 * live behind a `dist/` path. These local shapes cover exactly what the
 * assertions read, and keep the test off the library's internals.
 */
interface SchemaNode {
  required?: string[];
  properties?: Record<string, unknown>;
}
interface OperationNode {
  responses?: Record<string, { content?: Record<string, { schema?: unknown }> }>;
}

describe('openapi', () => {
  let app: INestApplication<App>;
  let document: OpenAPIObject;

  beforeAll(async () => {
    const moduleRef = await Test.createTestingModule({ imports: [AppModule] }).compile();

    app = moduleRef.createNestApplication({ logger: false });
    httpSetup(app, { globalPrefix: 'api' });
    setupOpenApi(app, { path: DOCS_PATH });
    await app.init();

    document = buildOpenApiDocument(app);
  });

  afterAll(async () => {
    await app.close();
  });

  describe('document', () => {
    it('carries the contract version, which is not the package version', () => {
      expect(document.info.title).toBe('Aave v4 Positions API');
      expect(document.info.version).toBe(API_CONTRACT_VERSION);
    });

    it('exposes exactly the routes we expect', () => {
      expect(Object.keys(document.paths).toSorted()).toEqual([
        '/api/hello',
        '/health/live',
        '/health/ready',
      ]);
    });

    /**
     * The drift guard. A route added without `@ApiOkResponse` still routes and
     * still appears in the document — but with an empty response and no schema,
     * so the published contract quietly stops describing what the service
     * returns. Nothing else fails when that happens.
     */
    it('gives every operation a typed success response', () => {
      const missing: string[] = [];

      for (const [path, item] of Object.entries(document.paths)) {
        for (const [method, operation] of Object.entries(item as Record<string, OperationNode>)) {
          const success = Object.entries(operation.responses ?? {}).find(([code]) =>
            code.startsWith('2'),
          );
          const schema = success?.[1].content?.['application/json']?.schema;

          if (!schema) missing.push(`${method.toUpperCase()} ${path}`);
        }
      }

      expect(missing).toEqual([]);
    });

    it('documents paths as deployed — probes outside the prefix, business inside', () => {
      const paths = Object.keys(document.paths);

      expect(paths.filter((p) => p.startsWith('/health/'))).toHaveLength(2);
      expect(paths.filter((p) => p.startsWith('/api/'))).toEqual(['/api/hello']);
    });

    it('describes both readiness outcomes, not just the happy path', () => {
      const responses = document.paths['/health/ready']?.get?.responses ?? {};

      expect(Object.keys(responses).toSorted()).toEqual(['200', '503']);
    });

    it('emits a real schema for the liveness response', () => {
      const schema = document.components?.schemas?.['LivenessResponseDto'] as SchemaNode;

      expect(schema.required?.toSorted()).toEqual(['status', 'uptimeSeconds']);
      expect(schema.properties?.['uptimeSeconds']).toMatchObject({ type: 'number' });
      expect(schema.properties?.['status']).toMatchObject({ enum: ['ok'] });
    });

    it('marks an optional property optional rather than required', () => {
      const schema = document.components?.schemas?.['CheckResultDto'] as SchemaNode;

      expect(schema.required?.toSorted()).toEqual(['name', 'status']);
      expect(schema.properties?.['error']).toBeDefined();
    });
  });

  describe('served surface', () => {
    it('serves the UI', async () => {
      const res = await request(app.getHttpServer()).get(`/${DOCS_PATH}`).expect(200);

      expect(res.headers['content-type']).toMatch(/html/);
    });

    it('serves the raw document as JSON', async () => {
      const res = await request(app.getHttpServer()).get(`/${DOCS_PATH}/openapi.json`).expect(200);

      expect(res.body.info.title).toBe('Aave v4 Positions API');
    });

    it('serves the raw document as YAML', async () => {
      const res = await request(app.getHttpServer()).get(`/${DOCS_PATH}/openapi.yaml`).expect(200);

      expect(res.text).toContain('Aave v4 Positions API');
    });
  });
});
