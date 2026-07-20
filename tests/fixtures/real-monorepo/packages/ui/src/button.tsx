import type { User } from '@fixture/contracts/models/user';
import { formatMoney } from '@fixture/shared';
import './button.css';

export const Button = (user: User) => `${user.id}: ${formatMoney(10)}`;
