import type { Order } from '@fixture/contracts/models/order';
import { currentUserVersion } from '@fixture/contracts';
import { loadOrders } from '@fixture/data';
import { Button } from '@fixture/ui/button';
import { localConfig } from '@app/config';
import { usedView } from './view-model';
import './styles/main.scss';

declare const require: (path: string) => unknown;

const checkout = import('./lazy-checkout');
const legacy = require('./legacy.cjs');
const routes = import.meta.glob('./routes/*.ts');
const hero = new URL('./assets/hero.svg', import.meta.url);

void ({} as Order);
void currentUserVersion;
void loadOrders;
void Button;
void localConfig;
void usedView;
void checkout;
void legacy;
void routes;
void hero;
