import type { Project } from '@/types/project';

export const CURRENT_SCHEMA_VERSION = 1;

export interface ProjectMigrationResult {
  project: Project;
  migrated: boolean;
  fromVersion: number;
  toVersion: number;
  appliedMigrations: string[];
}

export function migrateProject(project: Project): ProjectMigrationResult {
  const fromVersion = project.schemaVersion ?? 1;
  const migrated = fromVersion !== CURRENT_SCHEMA_VERSION;
  return {
    project: migrated ? { ...project, schemaVersion: CURRENT_SCHEMA_VERSION } : project,
    migrated,
    fromVersion,
    toVersion: CURRENT_SCHEMA_VERSION,
    appliedMigrations: migrated ? [`schema-v${fromVersion}-to-v${CURRENT_SCHEMA_VERSION}`] : [],
  };
}
