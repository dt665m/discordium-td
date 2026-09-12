import init, {schema_fixture} from './wasm-web/schema_web.js';
self.onmessage = async () => {
  try {
    const response = await fetch('./native.json');
    if (!response.ok) throw new Error('Native fixture is unavailable');
    const expected = await response.json();
    await init();
    const actual = JSON.parse(schema_fixture());
    const names = ['registry','global','owner','collision','hero','enemy','projectile','wisp','effect','damage','cover','cover_marker','platform'];
    const publicRoots = names.filter(name => !['registry','global','owner','collision'].includes(name));
    names.push(...publicRoots.map(name => `compact_${name}`));
    const valid = values => Array.isArray(values) && values.length === names.length && new Set(values.map(v => v[0])).size === names.length && values.every(v => names.includes(v[0]) && /^[0-9a-f]{64}$/.test(v[1]));
    if (!valid(expected) || !valid(actual)) throw new Error('Fixture roots or digest format are invalid');
    const values = new Map(actual);
    const rows = expected.map(([root,digest]) => ({root, expected:digest, actual:values.get(root), matches:digest===values.get(root)}));
    self.postMessage({passed:rows.every(row=>row.matches), roots:12, compact_public_roots:publicRoots.length, registry:values.get('registry'), rows});
  } catch (error) {
    self.postMessage({passed:false,error:String(error)});
  }
};
