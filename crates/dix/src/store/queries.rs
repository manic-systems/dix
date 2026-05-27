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
