(() => {
  const event = globalThis.__thinpack_event;

  module.exports = {
    Event: event.Event,
    CustomEvent: event.CustomEvent,
    EventTarget: event.EventTarget,
    defineEventHandler: event.defineEventHandler,
    isEventTarget: event.isEventTarget,
    kEvents: event.kEvents,
    kIsBeingDispatched: event.kIsBeingDispatched,
    kTarget: event.kTarget,
    kTrustEvent: event.kTrustEvent,
    kWeakHandler: event.kWeakHandler,
    kResistStopPropagation: event.kResistStopPropagation,
  };
})();
