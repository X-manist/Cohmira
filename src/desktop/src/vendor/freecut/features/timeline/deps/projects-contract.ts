import { create } from 'zustand';
import type { Project } from '@/types/project';

type ProjectMetadata = {
  width: number;
  height: number;
  fps: number;
  backgroundColor?: string;
};

type RedBoxProject = Pick<Project, 'id'>
  & Partial<Omit<Project, 'id' | 'metadata'>>
  & { metadata: ProjectMetadata };

type RedBoxProjectState = {
  projects: RedBoxProject[];
  currentProject: RedBoxProject | null;
  isLoading: boolean;
  error: string | null;
  searchQuery: string;
  sortField: 'updatedAt' | 'createdAt' | 'name';
  sortDirection: 'asc' | 'desc';
  filterResolution?: string;
  filterFps?: number;
};

type RedBoxProjectActions = {
  syncCurrentProject: (project: RedBoxProjectState['currentProject']) => void;
  setCurrentProject: (project: RedBoxProjectState['currentProject']) => void;
};

export const useProjectStore = create<RedBoxProjectState & RedBoxProjectActions>((set) => ({
  projects: [],
  isLoading: false,
  error: null,
  searchQuery: '',
  sortField: 'updatedAt',
  sortDirection: 'desc',
  filterResolution: undefined,
  filterFps: undefined,
  currentProject: {
    id: 'redbox-project',
    name: 'Employee project',
    description: '',
    createdAt: 0,
    updatedAt: 0,
    duration: 0,
    metadata: {
      width: 1080,
      height: 1920,
      fps: 30,
    },
  },
  syncCurrentProject: (currentProject) => set((state) => ({
    currentProject,
    projects: currentProject
      ? [...state.projects.filter((project) => project.id !== currentProject.id), currentProject]
      : state.projects,
  })),
  setCurrentProject: (currentProject) => set((state) => ({
    currentProject,
    projects: currentProject
      ? [...state.projects.filter((project) => project.id !== currentProject.id), currentProject]
      : state.projects,
  })),
}));

export function syncRedBoxTimelineProject(project: RedBoxProjectState['currentProject']) {
  useProjectStore.getState().syncCurrentProject(project);
}
