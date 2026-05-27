pub const QUERY_DEPENDENTS: &str = "
      WITH RECURSIVE
        graph(p) AS (
          SELECT id
          FROM ValidPaths
          WHERE path = ?
        UNION
          SELECT reference FROM Refs
          JOIN graph ON referrer = p
        )
      SELECT path from graph
      JOIN ValidPaths ON id = p;
    ";
pub const QUERY_SYSTEM_DERIVATIONS: &str = "
      WITH
        systemderiv AS (
          SELECT id FROM ValidPaths
          WHERE path = ?
        ),
        systempath AS (
          SELECT reference as id FROM systemderiv sd
          JOIN Refs ON sd.id = referrer
          JOIN ValidPaths vp ON reference = vp.id
          WHERE (vp.path LIKE '%-system-path')
        ),
        pkgs AS (
            SELECT reference as id FROM Refs
            JOIN systempath ON referrer = id
        )
      SELECT path FROM pkgs
      JOIN ValidPaths vp ON vp.id = pkgs.id;
    ";

pub const QUERY_CLOSURE_SIZE: &str = "
  WITH RECURSIVE
    graph(p) AS (
      SELECT id
      FROM ValidPaths
      WHERE path = ?
    UNION
      SELECT reference FROM Refs
      JOIN graph ON referrer = p
    )
  SELECT SUM(narSize) as sum from graph
  JOIN ValidPaths ON p = id;
";

pub const QUERY_PATH_SNAPSHOT: &str = "
  WITH RECURSIVE
    root(id) AS (
      SELECT id
      FROM ValidPaths
      WHERE path = ?1
    ),
    graph(id) AS (
      SELECT id
      FROM root
    UNION
      SELECT reference
      FROM Refs
      JOIN graph ON referrer = graph.id
    ),
    systempath(id) AS (
      SELECT reference
      FROM root
      JOIN Refs ON root.id = referrer
      JOIN ValidPaths ON ValidPaths.id = reference
      WHERE path LIKE '%-system-path'
    ),
    selected(id) AS (
      SELECT reference
      FROM Refs
      JOIN systempath ON referrer = systempath.id
    )
  SELECT 0 AS kind, path, narSize
  FROM graph
  JOIN ValidPaths ON ValidPaths.id = graph.id
UNION ALL
  SELECT 1 AS kind, path, NULL AS bytes
  FROM selected
  JOIN ValidPaths ON ValidPaths.id = selected.id
";
