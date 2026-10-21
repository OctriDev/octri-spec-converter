/** Complete, hand-authored sources shared by Rust and runtime qualification. */
import swagger from "../fixtures/launch/swagger.json" with { type: "json" };
import postman from "../fixtures/launch/postman.json" with { type: "json" };

export function launchCases() {
  return Object.entries({ swagger, postman }).flatMap(([adapter, fixtures]) =>
    fixtures.map((fixture) => ({
      adapter,
      fixture,
      request: {
        input: {
          uri: `memory://launch/${adapter}/${fixture.name}.json`,
          content: JSON.stringify(fixture.source),
          generated: false,
        },
        config: fixture.config ?? {},
        files: fixture.files ?? {},
      },
    })),
  );
}
