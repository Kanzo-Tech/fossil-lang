// The failure rules of `/docs/design/failure`, over every package's `src/`. A
// site that breaks one on purpose says why:
// `// eslint-disable-next-line no-restricted-syntax -- <reason>`.
import tseslint from 'typescript-eslint';

export default tseslint.config(
  { ignores: ['**/*.gen.ts', '**/*.d.ts'] },
  {
    files: ['packages/*/src/**/*.ts'],
    languageOptions: { parser: tseslint.parser },
    linterOptions: { reportUnusedDisableDirectives: 'error' },
    rules: {
      'no-empty': ['error', { allowEmptyCatch: false }],
      'no-restricted-syntax': [
        'error',
        {
          selector:
            "CallExpression[callee.property.name='catch'] > ArrowFunctionExpression.arguments:first-child:matches([body.type='BlockStatement'][body.body.length=0], [body.type='Identifier'][body.name='undefined'], [body.type='Literal'][body.raw='null'])",
          message: 'A rejection handler that answers nothing swallows the failure.',
        },
        {
          selector:
            "CallExpression[callee.property.name='then'] > ArrowFunctionExpression.arguments:nth-child(2):matches([body.type='Identifier'][body.name='undefined'], [body.type='Literal'][body.raw='null'])",
          message: 'A rejection handler that answers nothing swallows the failure.',
        },
        {
          selector:
            "AssignmentExpression[operator='??='][right.type=/^(CallExpression|AwaitExpression|NewExpression)$/]",
          message: '`??=` on a call memoizes a rejection; write the cache that clears itself.',
        },
        {
          selector:
            "CallExpression:matches([callee.name='fetch'], [callee.object.name='host'][callee.property.name=/^(connections|credentials)$/], [callee.object.name='job'][callee.property.name='complete']):not(CallExpression[callee.name='within'] *)",
          message: 'A host wait or a fetch goes through `within`.',
        },
      ],
    },
  },
);
