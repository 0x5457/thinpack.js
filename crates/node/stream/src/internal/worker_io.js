module.exports = {
  MessageChannel: class MessageChannel {
    constructor() {
      const error = new Error("MessageChannel is not implemented yet");
      error.code = "ERR_NOT_IMPLEMENTED";
      throw error;
    }
  },
};
