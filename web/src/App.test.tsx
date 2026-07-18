import { render, screen } from '@testing-library/react';
import { expect, test } from 'vitest';

import App from './App';

test('한국어 학습 목표와 핵심 탐색 경로를 제공한다', () => {
  render(<App />);

  expect(screen.getByRole('navigation', { name: '주요 메뉴' })).toBeInTheDocument();
  expect(screen.getByRole('heading', { name: '생각하는 힘을, 코드로 증명하세요' })).toBeInTheDocument();
  expect(screen.getByText('알고리즘 추론')).toBeInTheDocument();
  expect(screen.getByText('코드 읽기·디버깅')).toBeInTheDocument();
  expect(screen.getByText('문서 기반 구현')).toBeInTheDocument();
  expect(screen.getByText('독립 코딩')).toBeInTheDocument();
  expect(screen.getByText('메뉴')).toBeInTheDocument();
});
