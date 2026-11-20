/**
 * Whether the nexus is set up and we are signed in, and the guards that send
 * the app to setup or sign-in when needed.
 */

import { Injectable, effect, inject, signal } from '@angular/core';
import { type CanActivateFn, Router } from '@angular/router';
import type { NexusStatus, SignedIn } from './gen/api';
import { NexusHttp } from './http';
import { NexusSocket } from './socket';

const USER_KEY = 'shed.user';

@Injectable({ providedIn: 'root' })
export class AuthService {
  private readonly http = inject(NexusHttp);
  private readonly socket = inject(NexusSocket);
  private readonly router = inject(Router);

  readonly status = signal<NexusStatus | null>(null);
  readonly username = signal<string>(localStorage.getItem(USER_KEY) ?? '');
  /** This device's id, when known from signing in. */
  readonly device = signal<string | null>(null);

  constructor() {
    this.http.onUnauthorized = () => this.lost();
    effect(() => {
      const bye = this.socket.goodbye();
      if (bye && bye.code === 'unauthorized') this.lost();
    });
  }

  /** Asks the nexus; cached until signing in or out. */
  async check(force = false): Promise<NexusStatus> {
    const known = this.status();
    if (known && !force) return known;
    const status = await this.http.status();
    this.status.set(status);
    return status;
  }

  /** After signing in or finishing setup. */
  signedIn(s: SignedIn): void {
    this.http.setCsrf(s.csrf);
    this.username.set(s.username);
    this.device.set(s.device);
    localStorage.setItem(USER_KEY, s.username);
    const status = this.status();
    this.status.set({ ...(status ?? { version: '', api: 1 }), ready: true, signedIn: true });
    this.socket.connect();
  }

  async signOut(): Promise<void> {
    try {
      await this.http.logout();
    } catch {
      // Signed out anyway, as far as this browser goes.
    }
    this.forget();
    await this.router.navigateByUrl('/login');
  }

  /** The nexus no longer knows this browser (expired, revoked). */
  private lost(): void {
    this.forget();
    if (!this.router.url.startsWith('/login') && !this.router.url.startsWith('/setup')) {
      void this.router.navigate(['/login'], { queryParams: { expired: 1 } });
    }
  }

  private forget(): void {
    this.socket.disconnect();
    this.http.setCsrf(null);
    const status = this.status();
    if (status) this.status.set({ ...status, signedIn: false });
  }
}

/** Signed-in pages: to setup or sign-in when needed; opens the socket. */
export const authGuard: CanActivateFn = async () => {
  const auth = inject(AuthService);
  const router = inject(Router);
  try {
    const status = await auth.check();
    if (!status.ready) return router.parseUrl('/setup');
    if (!status.signedIn) return router.parseUrl('/login');
  } catch {
    // The nexus is not answering: show the app; the socket keeps trying.
  }
  inject(NexusSocket).connect();
  return true;
};

/** Setup and sign-in: away from them once done. */
export const setupGuard: CanActivateFn = async () => {
  const auth = inject(AuthService);
  const router = inject(Router);
  try {
    const status = await auth.check(true);
    if (status.ready) return router.parseUrl(status.signedIn ? '/' : '/login');
  } catch {
    // Show it; the call will say what is wrong.
  }
  return true;
};

export const loginGuard: CanActivateFn = async () => {
  const auth = inject(AuthService);
  const router = inject(Router);
  try {
    const status = await auth.check(true);
    if (!status.ready) return router.parseUrl('/setup');
    if (status.signedIn) return router.parseUrl('/');
  } catch {
    // Show it; signing in will say what is wrong.
  }
  return true;
};
