const express = require('express');
const fs = require('fs');
const path = require('path');
const app = express();
const PORT = 3000;

// env
const APP_ENV = process.env.APP_ENV || 'unknown';
const SECRET = process.env.SECRET_FROM_ENV || 'none';

app.get('/', (req, res) => {
  res.send(`m9 ok env=${APP_ENV} secret=${SECRET} ts=${Date.now()}`);
});

app.get('/health', (req, res) => {
  res.json({ status: 'ok', env: APP_ENV });
});

app.get('/pg', async (req, res) => {
  try {
    const { Client } = require('pg');
    const client = new Client({ host: process.env.PGHOST || 'postgres', user: process.env.PGUSER || 'postgres', password: process.env.PGPASSWORD || 'postgres', database: process.env.PGDATABASE || 'postgres', port: 5432 });
    await client.connect();
    const r = await client.query('SELECT 1 as ok');
    await client.end();
    res.json({ pg: r.rows[0].ok });
  } catch (e) { res.status(500).json({ error: e.message }); }
});

app.get('/redis', async (req, res) => {
  try {
    const redis = require('redis');
    const client = redis.createClient({ url: `redis://${process.env.REDIS_HOST || 'redis'}:6379` });
    await client.connect();
    await client.set('m9-key', 'm9-val');
    const v = await client.get('m9-key');
    await client.quit();
    res.json({ redis: v });
  } catch (e) { res.status(500).json({ error: e.message }); }
});

// for bind mount test: write file
app.get('/write', (req, res) => {
  const p = '/app/src/from-container.txt';
  fs.writeFileSync(p, 'from-container');
  res.send('written');
});

app.listen(PORT, '0.0.0.0', () => console.log(`m9 app listening ${PORT} env=${APP_ENV}`));
