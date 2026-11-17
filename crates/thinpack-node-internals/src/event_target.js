(() => {
  const event = globalThis.__thinpack_event;

  module.exports = {
    ...event,
    Event: event.Event,
    kEvents: Symbol("kEvents"),
    kIsBeingDispatched: event.kIsBeingDispatched,
    kTarget: event.kTarget,
    kTrustEvent: event.kTrustEvent,
    kResistStopPropagation: Symbol("kResistStopPropagation"),
    kWeakHandler: Symbol("kWeakHandler"),
    isEventTarget: (value) => value instanceof event.EventTarget,
  };
})();
