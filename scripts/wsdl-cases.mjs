/** WSDL wire methods retain case through every binding transport. */
export function wsdlMethodCases() {
  return ["GET", "get", "QUERY", "COPY"].flatMap((method) =>
    ["3.2.1", "3.1.2"].map((targetVersion) => ({
      method,
      targetVersion,
      request: {
        input: {
          uri: "memory://methods/service.wsdl",
          content: `<description xmlns="http://www.w3.org/ns/wsdl" xmlns:t="urn:methods" xmlns:h="http://www.w3.org/ns/wsdl/http" targetNamespace="urn:methods">
            <interface name="Api"><operation name="Call" pattern="http://www.w3.org/ns/wsdl/in-only"><input element="#none"/></operation></interface>
            <binding name="Http" interface="t:Api" type="http://www.w3.org/ns/wsdl/http" h:methodDefault="${method}"><operation ref="t:Call" h:location="call"/></binding>
            <service name="Api" interface="t:Api"><endpoint name="Http" binding="t:Http" address="https://api.example.test/v1/"/></service>
          </description>`,
          generated: false,
        },
        files: {},
        config: { targetVersion, apiVersion: "1" },
      },
    })),
  );
}

/** Explicit and default-only WSDL 2.0 bindings share one engine contract. */
export function wsdlDefaultCases() {
  return [
    "http",
    "soap-post",
    "soap-get",
    "soap-headers",
    "soap-fault",
    "override",
  ].map((mode) => {
    const soap = mode.startsWith("soap-");
    const protocol = soap ? "soap" : "http";
    const defaults = soap
      ? `s:protocol="http://www.w3.org/2003/05/soap/bindings/HTTP/" ${mode === "soap-get" ? 's:mepDefault="http://www.w3.org/2003/05/soap/mep/soap-response/"' : ""}`
      : "";
    const override =
      mode === "soap-fault"
        ? '<fault ref="t:Failed"><s:header element="t:Auth" required="true"/></fault>'
        : mode === "soap-headers"
          ? '<operation ref="t:Read"><input><s:header element="t:Auth" required="true"/></input><output><s:header element="t:Auth"/></output></operation>'
          : mode === "override"
            ? '<operation ref="t:Read" h:method="DELETE" h:location="read"/>'
            : "";
    return {
      mode,
      request: {
        input: {
          uri: "memory://defaults/service.wsdl",
          content: `<description xmlns="http://www.w3.org/ns/wsdl" xmlns:t="urn:defaults" xmlns:h="http://www.w3.org/ns/wsdl/http" xmlns:s="http://www.w3.org/ns/wsdl/soap" xmlns:x="http://www.w3.org/2001/XMLSchema" xmlns:wx="http://www.w3.org/ns/wsdl-extensions" targetNamespace="urn:defaults">
            ${mode === "soap-headers" || mode === "soap-fault" ? '<types><x:schema targetNamespace="urn:defaults"><x:element name="Auth" type="x:string"/></x:schema></types>' : ""}
            <interface name="Api">${mode === "soap-fault" ? '<fault name="Failed" element="t:Auth"/>' : ""}<operation name="Read" pattern="http://www.w3.org/ns/wsdl/in-out" wx:safe="true"><input element="#none"/><output element="#none"/>${mode === "soap-fault" ? '<outfault ref="t:Failed"/>' : ""}</operation></interface>
            <binding name="Binding" interface="t:Api" type="http://www.w3.org/ns/wsdl/${protocol}" ${defaults}>${override}</binding>
            <service name="Api" interface="t:Api"><endpoint name="Main" binding="t:Binding" address="https://api.example.test/v1/" h:authenticationScheme="basic"/></service>
          </description>`,
          generated: false,
        },
        files: {},
        config: { targetVersion: "3.2.1", apiVersion: "1" },
      },
    };
  });
}
