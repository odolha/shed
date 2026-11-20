import { Routes } from '@angular/router';
import { authGuard, loginGuard, setupGuard } from '../api/auth';

export const routes: Routes = [
  {
    path: 'setup',
    canActivate: [setupGuard],
    loadComponent: () => import('./setup/setup').then((m) => m.SetupPage),
  },
  {
    path: 'login',
    canActivate: [loginGuard],
    loadComponent: () => import('./login/login').then((m) => m.LoginPage),
  },
  {
    path: '',
    canActivate: [authGuard],
    loadComponent: () => import('./shell/shell').then((m) => m.Shell),
    children: [
      { path: '', loadComponent: () => import('./workspaces/overview').then((m) => m.Overview) },
      { path: 'w/:workspace', loadComponent: () => import('./workspaces/overview').then((m) => m.Overview) },
      {
        path: 'w/:workspace/logbook',
        loadComponent: () => import('./logbook/logbook-page').then((m) => m.LogbookPage),
      },
      {
        path: 'w/:workspace/b/:bench',
        loadComponent: () => import('./bench/bench-page').then((m) => m.BenchPage),
      },
      {
        path: 'w/:workspace/b/:bench/t/:tab',
        loadComponent: () => import('./bench/bench-page').then((m) => m.BenchPage),
      },
      { path: 'devices', loadComponent: () => import('./devices/devices').then((m) => m.DevicesPage) },
      { path: 'settings', loadComponent: () => import('./settings/settings').then((m) => m.SettingsPage) },
    ],
  },
  { path: '**', redirectTo: '' },
];
